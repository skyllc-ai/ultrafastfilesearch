// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! `SearchParams` builder/resolver methods.

use super::{
    SearchFilterMode, SearchParams, SearchPredicate, SearchPredicateOp, SearchPredicateValue,
    SearchResponseMode, SearchSortDirection, SearchSortSpec,
};

impl SearchParams {
    /// Resolve the effective filter mode, preferring the canonical field.
    #[must_use]
    pub fn resolved_filter_mode(&self) -> SearchFilterMode {
        self.filter_mode.unwrap_or(match self.filter.as_deref() {
            Some("files") => SearchFilterMode::Files,
            Some("dirs") => SearchFilterMode::Dirs,
            _ => SearchFilterMode::All,
        })
    }

    /// Resolve the effective sort clauses, preferring the canonical vector.
    #[must_use]
    pub fn resolved_sorts(&self) -> Vec<SearchSortSpec> {
        if self.sorts.is_empty() {
            self.sort.as_deref().map_or_else(Vec::new, |sort| {
                Self::canonicalize_legacy_sort(sort, self.sort_desc)
            })
        } else {
            self.sorts.clone()
        }
    }

    /// Resolve the effective canonical predicate list.
    #[must_use]
    pub(crate) fn resolved_predicates(&self) -> Vec<SearchPredicate> {
        if !self.predicates.is_empty() {
            return self.predicates.clone();
        }

        let mut predicates = Vec::new();
        self.push_bound_predicates(&mut predicates);
        self.push_legacy_time_predicates(&mut predicates);
        self.push_extension_and_exclude(&mut predicates);
        self.push_attr_predicates(&mut predicates);
        self.push_malformed_predicates(&mut predicates);

        // NOTE: `hide_system` is NOT emitted as a predicate.  It is already
        // compiled into the hot-path `SearchFilters.hide_system` flag by
        // `SearchFilters::from_params`.  Emitting a "system_name" predicate
        // here would cause `requires_post_filter = true` (unknown field) →
        // limit removal → full scan with DisplayRow construction for every
        // record (~22 s on 25 M records instead of ~100 ms).

        predicates
    }

    /// Push size and descendant bound predicates from legacy fields.
    fn push_bound_predicates(&self, predicates: &mut Vec<SearchPredicate>) {
        if let Some(min_size) = self.min_size {
            predicates.push(SearchPredicate {
                field: "size".to_owned(),
                op: SearchPredicateOp::Gte,
                value: SearchPredicateValue::U64(min_size),
            });
        }
        if let Some(max_size) = self.max_size {
            predicates.push(SearchPredicate {
                field: "size".to_owned(),
                op: SearchPredicateOp::Lte,
                value: SearchPredicateValue::U64(max_size),
            });
        }
        if let Some(min_descendants) = self.min_descendants {
            predicates.push(SearchPredicate {
                field: "descendants".to_owned(),
                op: SearchPredicateOp::Gte,
                value: SearchPredicateValue::U64(u64::from(min_descendants)),
            });
        }
        if let Some(max_descendants) = self.max_descendants {
            predicates.push(SearchPredicate {
                field: "descendants".to_owned(),
                op: SearchPredicateOp::Lte,
                value: SearchPredicateValue::U64(u64::from(max_descendants)),
            });
        }
    }

    /// Push all six legacy time-bound predicates (newer/older ×
    /// modified/created/accessed).
    fn push_legacy_time_predicates(&self, predicates: &mut Vec<SearchPredicate>) {
        for (field, op, spec) in [
            ("modified", SearchPredicateOp::Gte, self.newer.as_deref()),
            ("modified", SearchPredicateOp::Lt, self.older.as_deref()),
            (
                "created",
                SearchPredicateOp::Gte,
                self.newer_created.as_deref(),
            ),
            (
                "created",
                SearchPredicateOp::Lt,
                self.older_created.as_deref(),
            ),
            (
                "accessed",
                SearchPredicateOp::Gte,
                self.newer_accessed.as_deref(),
            ),
            (
                "accessed",
                SearchPredicateOp::Lt,
                self.older_accessed.as_deref(),
            ),
        ] {
            if let Some(val) = spec {
                predicates.push(SearchPredicate {
                    field: field.to_owned(),
                    op,
                    value: SearchPredicateValue::String(val.to_owned()),
                });
            }
        }
    }

    /// Push extension filter and exclude predicates from legacy fields.
    fn push_extension_and_exclude(&self, predicates: &mut Vec<SearchPredicate>) {
        if let Some(ext) = self.ext.as_deref() {
            let values = ext
                .split(',')
                .map(|segment| segment.trim().trim_start_matches('.').to_owned())
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>();
            if !values.is_empty() {
                predicates.push(SearchPredicate {
                    field: "extension".to_owned(),
                    op: SearchPredicateOp::In,
                    value: SearchPredicateValue::StringList(values),
                });
            }
        }

        if let Some(exclude) = self.exclude.as_ref() {
            predicates.push(SearchPredicate {
                field: "name".to_owned(),
                op: SearchPredicateOp::NotMatch,
                value: SearchPredicateValue::String(exclude.clone()),
            });
        }
    }

    /// Push attribute require/exclude predicates from legacy `--attr` flag.
    fn push_attr_predicates(&self, predicates: &mut Vec<SearchPredicate>) {
        let mut required = Vec::new();
        let mut excluded = Vec::new();
        if let Some(attr) = self.attr.as_deref() {
            for part in attr.split(',') {
                let trimmed = part.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Some(name) = trimmed.strip_prefix('!') {
                    excluded.push(name.to_ascii_lowercase());
                } else {
                    required.push(trimmed.to_ascii_lowercase());
                }
            }
        }
        if !required.is_empty() {
            predicates.push(SearchPredicate {
                field: "attributes".to_owned(),
                op: SearchPredicateOp::HasAll,
                value: SearchPredicateValue::StringList(required),
            });
        }
        if !excluded.is_empty() {
            predicates.push(SearchPredicate {
                field: "attributes".to_owned(),
                op: SearchPredicateOp::HasNone,
                value: SearchPredicateValue::StringList(excluded),
            });
        }
    }

    /// Push the WI-4.4 malformed-name predicates from `--malformed` /
    /// `--malformed-path`. `malformed` (leaf) compiles into the hot path on the
    /// daemon; `malformed_path` is post-filtered (it is path-derived).
    fn push_malformed_predicates(&self, predicates: &mut Vec<SearchPredicate>) {
        if let Some(want) = self.malformed {
            predicates.push(SearchPredicate {
                field: "malformed".to_owned(),
                op: SearchPredicateOp::Eq,
                value: SearchPredicateValue::Bool(want),
            });
        }
        if let Some(want) = self.malformed_path {
            predicates.push(SearchPredicate {
                field: "malformed_path".to_owned(),
                op: SearchPredicateOp::Eq,
                value: SearchPredicateValue::Bool(want),
            });
        }
    }

    /// Resolve the requested response mode.
    #[must_use]
    pub fn resolved_response_mode(&self) -> SearchResponseMode {
        self.response_mode.unwrap_or(SearchResponseMode::Rows)
    }

    /// Fill additive canonical fields from legacy fields in one shared place.
    pub fn populate_canonical_fields(&mut self) {
        if self.filter_mode.is_none() {
            self.filter_mode = Some(self.resolved_filter_mode());
        }
        if self.sorts.is_empty() {
            self.sorts = self.resolved_sorts();
        }
        if self.predicates.is_empty() {
            self.predicates = self.resolved_predicates();
        }
        if self.response_mode.is_none() {
            self.response_mode = Some(self.resolved_response_mode());
        }
    }

    /// Canonicalize a legacy comma-separated sort string plus `sort_desc` flag.
    ///
    /// Supports three direction syntaxes:
    /// - Prefix: `-size` means descending, bare `size` means ascending
    /// - Suffix: `size:desc` or `size:asc` (explicit)
    /// - Flag:   `--sort-desc` flips the first field to descending
    ///
    /// **First field:** ascending by default; descending if prefixed with `-`
    /// or if `sort_desc` is true.
    ///
    /// **Secondary fields:** use field-type defaults (numeric/time → desc,
    /// string → asc) unless overridden with prefix or suffix.
    #[must_use]
    pub(crate) fn canonicalize_legacy_sort(sort: &str, sort_desc: bool) -> Vec<SearchSortSpec> {
        sort.split(',')
            .enumerate()
            .filter_map(|(index, raw_part)| {
                let trimmed = raw_part.trim();
                if trimmed.is_empty() {
                    return None;
                }

                // Check for `-` prefix (e.g. "-modified" → descending).
                let (has_dash_prefix, after_dash) = trimmed
                    .strip_prefix('-')
                    .map_or((false, trimmed), |rest| (true, rest));

                let (field, explicit_direction) = after_dash
                    .split_once(':')
                    .map_or((after_dash, None), |(lhs, rhs)| {
                        (lhs.trim(), Some(rhs.trim()))
                    });

                // Parse explicit suffix direction token (e.g. "size:desc").
                let parsed_dir = explicit_direction.and_then(|dir| {
                    match dir.trim().to_ascii_lowercase().as_str() {
                        "asc" | "ascending" => Some(SearchSortDirection::Asc),
                        "desc" | "descending" => Some(SearchSortDirection::Desc),
                        _ => None,
                    }
                });

                // Resolve direction: suffix > prefix > flag (first field) > default.
                let direction = parsed_dir.or_else(|| {
                    if has_dash_prefix {
                        return Some(SearchSortDirection::Desc);
                    }
                    if index == 0 {
                        // First field: ascending unless --sort-desc is set.
                        return Some(if sort_desc {
                            SearchSortDirection::Desc
                        } else {
                            SearchSortDirection::Asc
                        });
                    }
                    // Secondary fields: field-type default.
                    Some(match field.trim().to_ascii_lowercase().as_str() {
                        "size" | "sizeondisk" | "size_on_disk" | "allocated" | "created"
                        | "modified" | "written" | "date" | "accessed" | "descendants"
                        | "treesize" | "tree_size" | "treeallocated" | "tree_allocated" => {
                            SearchSortDirection::Desc
                        }
                        _ => SearchSortDirection::Asc,
                    })
                });

                Some(SearchSortSpec {
                    field: field.to_owned(),
                    direction,
                })
            })
            .collect()
    }
}
