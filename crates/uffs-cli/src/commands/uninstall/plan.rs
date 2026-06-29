// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Removal-plan construction for `uffs --uninstall` (task U-20 of
//! `docs/dev/architecture/UFFS-Uninstall-Implementation-Plan.md`).
//!
//! Pure: turns the analysis ([`DetectionReport`] + [`Inventory`]) into an
//! ordered, itemized [`RemovalPlan`], honoring `--keep-config` / `--scope`.
//! No IO, fully unit-tested. `WinGet` roots become a `winget uninstall`
//! delegation, never a hand-delete (design §7).

use super::args::{UninstallArgs, UninstallScope};
use super::inventory::{ArtifactKind, BrokerServiceState, Inventory};
use crate::commands::update::model::{Channel, DetectionReport, InstallRoot, Scope};

/// What a plan item does to its target. Ordering of the variants mirrors the
/// safe removal order (stop before delete; self-delete is handled later).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Stop a running process (daemon / MCP gateway).
    StopProcess,
    /// Stop + delete the broker Windows service.
    RemoveService,
    /// Delete UFFS binaries in an unmanaged / dev-build root.
    DeleteBinaries,
    /// Hand the root to `winget uninstall` (never hand-deleted).
    DelegateWinget,
    /// Recursively delete a data / cache / config directory.
    DeleteDir,
}

impl Action {
    /// Short verb label (used in `--json`).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::StopProcess => "stop-process",
            Self::RemoveService => "remove-service",
            Self::DeleteBinaries => "delete-binaries",
            Self::DelegateWinget => "delegate-winget",
            Self::DeleteDir => "delete-dir",
        }
    }
}

/// Coarse scope of a plan item, for `--scope` filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemScope {
    /// A per-user artifact (`%LOCALAPPDATA%`, a user-scope root).
    User,
    /// A machine-wide artifact (the service, a `%PROGRAMFILES%` root).
    Machine,
    /// Scope-agnostic (a running process).
    Any,
}

/// One unit of removal work.
#[derive(Debug, Clone)]
pub(crate) struct PlanItem {
    /// What this item does.
    pub(crate) action: Action,
    /// Human description of the target.
    pub(crate) description: String,
    /// Whether performing it requires Administrator.
    pub(crate) needs_elevation: bool,
    /// Coarse scope, for `--scope` filtering.
    pub(crate) scope: ItemScope,
    /// Bytes this item reclaims (0 for non-filesystem actions).
    pub(crate) bytes: u64,
}

/// A named, ordered group of plan items (Services, Processes, ...).
#[derive(Debug, Clone)]
pub(crate) struct PlanGroup {
    /// Group heading.
    pub(crate) title: &'static str,
    /// Items in the group.
    pub(crate) items: Vec<PlanItem>,
}

/// The full ordered removal plan.
#[derive(Debug, Clone, Default)]
pub(crate) struct RemovalPlan {
    /// Groups in safe removal order.
    pub(crate) groups: Vec<PlanGroup>,
}

impl RemovalPlan {
    /// Iterate every item across all groups.
    fn items(&self) -> impl Iterator<Item = &PlanItem> {
        self.groups.iter().flat_map(|group| &group.items)
    }

    /// Total bytes the plan would reclaim.
    pub(crate) fn total_bytes(&self) -> u64 {
        self.items()
            .map(|item| item.bytes)
            .fold(0, u64::saturating_add)
    }

    /// Whether any item requires Administrator.
    pub(crate) fn requires_elevation(&self) -> bool {
        self.items().any(|item| item.needs_elevation)
    }

    /// Number of items across all groups.
    pub(crate) fn item_count(&self) -> usize {
        self.groups.iter().map(|group| group.items.len()).sum()
    }

    /// True when there is nothing to remove.
    pub(crate) fn is_empty(&self) -> bool {
        self.item_count() == 0
    }
}

/// Build the ordered removal plan from the analysis + flags.
pub(crate) fn build_plan(
    report: &DetectionReport,
    inventory: &Inventory,
    args: &UninstallArgs,
) -> RemovalPlan {
    let mut groups: Vec<PlanGroup> = Vec::new();

    // 1. Services (the broker, elevated) — removed first conceptually.
    if inventory.broker_service == BrokerServiceState::Installed {
        let item = PlanItem {
            action: Action::RemoveService,
            description: format!(
                "Stop + delete service {}",
                uffs_broker_protocol::SERVICE_NAME
            ),
            needs_elevation: true,
            scope: ItemScope::Machine,
            bytes: 0,
        };
        push_group(&mut groups, "Services", vec![item], args.scope);
    }

    // 2. Processes (stopped before their binaries are deleted).
    let processes: Vec<PlanItem> = report
        .running
        .iter()
        .map(|process| PlanItem {
            action: Action::StopProcess,
            description: format!("{} (pid {})", process.component.label(), process.pid),
            needs_elevation: false,
            scope: ItemScope::Any,
            bytes: 0,
        })
        .collect();
    push_group(
        &mut groups,
        "Processes (stopped first)",
        processes,
        args.scope,
    );

    // 3. Binaries — per root: unmanaged/dev delete, winget delegate.
    let binaries: Vec<PlanItem> = report.roots.iter().filter_map(binary_item).collect();
    push_group(&mut groups, "Binaries", binaries, args.scope);

    // 4. Data / cache / config dirs that exist (skip config under --keep-config).
    let dirs: Vec<PlanItem> = inventory
        .dirs
        .iter()
        .filter(|dir| dir.exists)
        .filter(|dir| !(args.keep_config && dir.kind == ArtifactKind::Config))
        .map(|dir| PlanItem {
            action: Action::DeleteDir,
            description: format!("{} ({})", dir.kind.label(), dir.path.display()),
            needs_elevation: false,
            scope: ItemScope::User,
            bytes: dir.size_bytes,
        })
        .collect();
    push_group(&mut groups, "Data / cache / config", dirs, args.scope);

    RemovalPlan { groups }
}

/// Build the per-root binary plan item, or `None` for an empty root.
fn binary_item(root: &InstallRoot) -> Option<PlanItem> {
    if root.binaries.is_empty() {
        return None;
    }
    let machine = matches!(root.scope, Scope::Machine);
    let scope = if machine {
        ItemScope::Machine
    } else {
        ItemScope::User
    };
    let item = match root.channel {
        Channel::WinGet => PlanItem {
            action: Action::DelegateWinget,
            description: format!("winget uninstall SkyLLC.UFFS  ({})", root.dir.display()),
            needs_elevation: machine,
            scope,
            bytes: 0,
        },
        Channel::Unmanaged | Channel::DevBuild | Channel::Unknown => PlanItem {
            action: Action::DeleteBinaries,
            description: format!("{} binaries in {}", root.binaries.len(), root.dir.display()),
            needs_elevation: machine,
            scope,
            bytes: 0,
        },
    };
    Some(item)
}

/// Apply the `--scope` filter and append the group only if it has items left.
fn push_group(
    groups: &mut Vec<PlanGroup>,
    title: &'static str,
    items: Vec<PlanItem>,
    scope: UninstallScope,
) {
    let kept: Vec<PlanItem> = items
        .into_iter()
        .filter(|item| scope_admits(scope, item.scope))
        .collect();
    if !kept.is_empty() {
        groups.push(PlanGroup { title, items: kept });
    }
}

/// Whether a `--scope` request admits an item of the given scope.
const fn scope_admits(requested: UninstallScope, item: ItemScope) -> bool {
    match (requested, item) {
        (UninstallScope::All, _)
        | (_, ItemScope::Any)
        | (UninstallScope::User, ItemScope::User)
        | (UninstallScope::Machine, ItemScope::Machine) => true,
        (UninstallScope::User, ItemScope::Machine) | (UninstallScope::Machine, ItemScope::User) => {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Action, RemovalPlan, build_plan};
    use crate::commands::uninstall::args::{UninstallArgs, UninstallScope};
    use crate::commands::uninstall::inventory::{
        ArtifactDir, ArtifactKind, BrokerServiceState, Inventory,
    };
    use crate::commands::update::model::{
        BinaryInfo, Channel, Component, DetectionReport, InstallRoot, RunningProcess, Scope,
    };

    fn root(channel: Channel, scope: Scope, dir: &str) -> InstallRoot {
        InstallRoot {
            dir: PathBuf::from(dir),
            channel,
            scope,
            anchored_by: Vec::new(),
            binaries: vec![BinaryInfo {
                name: "uffs".to_owned(),
                version: Some("0.6.16".to_owned()),
            }],
        }
    }

    fn inventory(broker: BrokerServiceState, config_size: u64) -> Inventory {
        Inventory {
            dirs: vec![
                ArtifactDir {
                    kind: ArtifactKind::Cache,
                    path: PathBuf::from("/x/cache"),
                    exists: true,
                    size_bytes: 2048,
                },
                ArtifactDir {
                    kind: ArtifactKind::Config,
                    path: PathBuf::from("/x/config"),
                    exists: true,
                    size_bytes: config_size,
                },
            ],
            broker_service: broker,
        }
    }

    fn find_action(plan: &RemovalPlan, action: Action) -> bool {
        plan.groups
            .iter()
            .flat_map(|group| &group.items)
            .any(|item| item.action == action)
    }

    #[test]
    fn winget_root_is_delegated_not_deleted() {
        let report = DetectionReport {
            roots: vec![root(Channel::WinGet, Scope::User, r"C:\winget\uffs")],
            running: Vec::new(),
        };
        let plan = build_plan(
            &report,
            &inventory(BrokerServiceState::Absent, 1024),
            &UninstallArgs::default(),
        );
        assert!(find_action(&plan, Action::DelegateWinget));
        assert!(!find_action(&plan, Action::DeleteBinaries));
    }

    #[test]
    fn machine_root_needs_elevation() {
        let report = DetectionReport {
            roots: vec![root(
                Channel::Unmanaged,
                Scope::Machine,
                r"C:\Program Files\uffs",
            )],
            running: Vec::new(),
        };
        let plan = build_plan(
            &report,
            &inventory(BrokerServiceState::Absent, 1024),
            &UninstallArgs::default(),
        );
        assert!(plan.requires_elevation());
    }

    #[test]
    fn service_present_requires_elevation_and_is_first() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: Vec::new(),
        };
        let plan = build_plan(
            &report,
            &inventory(BrokerServiceState::Installed, 1024),
            &UninstallArgs::default(),
        );
        assert!(plan.requires_elevation());
        assert!(find_action(&plan, Action::RemoveService));
        assert_eq!(plan.groups.first().expect("a group").title, "Services");
    }

    #[test]
    fn keep_config_drops_the_config_dir() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: Vec::new(),
        };
        let inv = inventory(BrokerServiceState::Absent, 4096);
        let with_config = build_plan(&report, &inv, &UninstallArgs::default());
        let keep = UninstallArgs {
            keep_config: true,
            ..UninstallArgs::default()
        };
        let without_config = build_plan(&report, &inv, &keep);
        assert!(with_config.total_bytes() > without_config.total_bytes());
    }

    #[test]
    fn scope_user_excludes_the_machine_service() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: Vec::new(),
        };
        let user_only = UninstallArgs {
            scope: UninstallScope::User,
            ..UninstallArgs::default()
        };
        let plan = build_plan(
            &report,
            &inventory(BrokerServiceState::Installed, 1024),
            &user_only,
        );
        assert!(!find_action(&plan, Action::RemoveService));
        assert!(!plan.requires_elevation());
    }

    #[test]
    fn running_process_becomes_a_stop_item() {
        let report = DetectionReport {
            roots: Vec::new(),
            running: vec![RunningProcess {
                component: Component::Daemon,
                pid: 4242,
                image_path: None,
                command_line: None,
                version: None,
            }],
        };
        let plan = build_plan(
            &report,
            &inventory(BrokerServiceState::Absent, 1024),
            &UninstallArgs::default(),
        );
        assert!(find_action(&plan, Action::StopProcess));
    }
}
