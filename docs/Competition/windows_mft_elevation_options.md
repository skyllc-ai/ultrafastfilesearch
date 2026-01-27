# Designing a user-friendly “instant file search” that reads the NTFS MFT (and therefore needs elevation)

## Executive summary (what I’d ship)

**Most user-friendly default:**

1) The main app (UI/CLI) runs **non-elevated**.
2) Offer an optional, tiny **Windows service** (“Fast Index Helper”) that performs privileged operations (open volume handles, enumerate MFT/USN, maintain index), and exposes a *minimal* IPC interface to the non-elevated UI.
3) Provide a **no-admin fallback mode** that uses slower, unprivileged mechanisms (Win32 file APIs and/or Windows Search index when available).

This is the same general pattern used by other Windows search/indexing solutions:
- **Everything**: prompts users to either run indexing elevated or install the “Everything service” so the UI can remain a standard user process.  
- **Windows Search**: runs an indexer as a background service and serves results back to users while “access restrictions still apply” (security trimming).

In practice, this “install once, run normally forever” path produces the fewest UAC prompts and the least user confusion.

---

## 1) Why MFT enumeration pushes you into elevation

### 1.1 What you’re doing (at a system level)

Typical MFT/USN enumeration approaches:
- Open a **volume handle** like `\\.\C:` and call `DeviceIoControl` with `FSCTL_ENUM_USN_DATA` to enumerate MFT-backed USN data (the docs explicitly describe using a volume handle and this control code to obtain MFT records).
- Optionally track changes via the USN Journal (`FSCTL_READ_USN_JOURNAL`) once you’ve built an initial index.

Microsoft’s `FSCTL_ENUM_USN_DATA` documentation describes enumerating USN data to obtain **MFT records** and doing so via a handle to `\\.\X:`.  

### 1.2 Why Windows requires admin rights / special privileges

Windows treats opening disks/volumes for low-level operations as sensitive. Microsoft’s CreateFile3 documentation explicitly lists the requirements for opening a volume/disk device handle and includes: **“The caller must have administrative privileges.”** (This is exactly the kind of handle you need for MFT/USN enumeration).

Even if you can technically obtain a handle in some edge configurations, you should design assuming:
- Standard users often cannot get the handle you need.
- Enterprises lock this down even harder.
- Future Windows hardening trends (ex: Administrator Protection work) make “just run elevated all the time” an increasingly brittle UX choice.

### 1.3 The “hidden” security problem: metadata leaks

If your privileged component can read the MFT, it can “see” file names/paths across the volume, including for locations the user cannot normally list. If you return those results directly to a non-elevated user, you’ve built a **metadata disclosure** channel (at minimum: filenames, paths, timestamps; potentially more).

So “make it work” is not enough—**you must decide whether and how to security-trim results**.

---

## 2) UX goals and constraints (define these early)

**User-friendly** in Windows usually means:
- Minimal UAC prompts.
- Predictable behavior (no surprise “Access Denied” after it seemed to work).
- Clear choices with safe defaults.
- Works for:
  - Admin users (home power users).
  - Standard users (common in enterprises, schools).
  - Locked-down endpoints (no service installation by the user).

**Security** means:
- No privilege escalation paths through your helper/service.
- No unintended cross-user data leaks.
- Least privilege and attack-surface minimization.

**Performance** means:
- Fast query time (your differentiator).
- Efficient incremental updates (USN Journal is the usual strategy once indexed).

---

## 3) Options for handling elevation (deep dive)

### Quick comparison

| Option | UAC prompts | Works for standard users (no admin) | Performance | Security risk | Typical use |
|---|---:|---:|---:|---:|---|
| A. Always run the whole app elevated | Frequent | No | Excellent | Highest (big elevated UI surface) | Utilities / “admin tools” |
| B. Elevate only the indexing sub-process (on demand) | Frequent-ish | No | Excellent | Medium | Portable tools |
| C. Install a small Windows service (helper) | One-time | Often **Yes** (once installed by admin/IT) | Excellent | Low if hardened | Best UX overall |
| D. Scheduled task “run highest” helper | One-time-ish | No (unless preprovisioned) | Excellent | Medium (complexity) | Niche workaround |
| E. Kernel driver / minifilter | One-time | Needs admin/IT | Excellent | Very high (driver risk) | Security/forensics products |
| F. No-elevation mode (Win32 APIs / Windows Search) | None | Yes | Varies | Low | Default fallback |

---

### Option A — Run the whole app elevated (manifest `requireAdministrator`)

**What it looks like:** user launches your app and immediately gets a UAC consent prompt. Every run.

**Pros**
- Simplest engineering.
- Least moving parts.

**Cons (big ones)**
- Worst UX: frequent UAC prompts train users to click “Yes” without thinking.
- Elevated UI is a large attack surface (plugins, file previews, rich text, shell integrations).
- Hard to operate safely with untrusted input (paths, queries, config files).
- In locked-down environments, the tool simply won’t run for standard users.

**When it’s acceptable**
- You’re explicitly an “admin/forensics” tool.
- Target users are power users who expect elevation.

---

### Option B — Split into non-elevated UI + elevated helper *process* (UAC each session)

**What it looks like:**
- UI runs normally.
- First time you need MFT access, UI launches a separate helper EXE with `runas` (UAC prompt).
- Helper runs for the session and exits when UI exits.

This is the pattern Everything recommends for portable usage: “Run indexing process as administrator” as an alternative to installing a service, but with UAC prompts each time.

**Pros**
- No persistent installation.
- UI stays non-elevated (good).
- Good performance.

**Cons**
- Still frequent UAC prompts (every launch or every “enable instant search” action).
- Inter-process protocol must be secure (you’re building your own IPC RPC surface anyway).
- Harder to support multi-user or background indexing (helper is tied to a session).

**When it’s best**
- Truly portable distribution (no installer, no service install).
- Users tolerate UAC prompts (power user audience).

---

### Option C — Install a small Windows service (recommended “golden path”)

**What it looks like:**
- UI runs non-elevated.
- On first run (or first “Enable Instant Search”), user is offered:
  - **Install Helper Service (recommended)** — requires admin once.
  - **Use slower mode** — no admin needed.
- Service runs as a background component to build/maintain the index and answer queries.

This is directly aligned with how Everything positions its service:
- “Everything can run as a standard user when the Everything service is installed.”
- On modern Windows with a standard user account, installing the service is required to use NTFS indexing.

**Pros**
- Best UX: **one-time** UAC prompt.
- UI remains non-elevated.
- Service can keep the index warm and up-to-date, even if UI isn’t running.
- You can harden the service heavily (restricted token, no network, minimal privileges).

**Cons / costs**
- Installer complexity (MSI/MSIX + service installation).
- Requires admin/IT to install service (but that’s a one-time provisioning problem, solvable via enterprise deployment).
- You must do security trimming thoughtfully.

**Security design checklist (do not skip)**
1) **Narrow responsibilities:** service only does:
   - enumerate NTFS metadata,
   - maintain index,
   - answer queries,
   - optionally resolve FileReferenceNumbers to paths.
2) **Minimal IPC surface:** define a tiny protocol: `QueryIndex`, `GetStatus`, `RescanVolume`.
3) **Authenticate callers:** use named pipes with a restrictive security descriptor; allow only:
   - local interactive users, and/or
   - a specific local group you create at install time.
   Microsoft documents that named pipes support security descriptors and access control.
4) **Impersonate and security-trim:** where feasible, impersonate the client and/or perform `AccessCheck` logic so results don’t leak filenames a caller couldn’t normally enumerate.
   - A performance-friendly approach is to store security info in the index and do “trimming” at query time, similar to how search systems apply access restrictions.
5) **Service hardening:**
   - Run as a dedicated service account when possible (or LocalService) and grant only necessary privileges; avoid full LocalSystem unless required.
   - If you must use LocalSystem for volume access, compensate with service hardening and very strict IPC.
   - Disable outbound network access if you don’t need it (service SID isolation, firewall rules).
6) **Update path:** ensure the service can be updated safely (signed binaries, secure update mechanism).

**How Windows Search does it (conceptually)**
Windows Search maintains a shared index “with security restrictions on content access” and policies state that when indexing decrypted content, “access restrictions will still apply.” This is the model you want: privileged indexing, non-privileged querying with security trimming.

---

### Option D — Scheduled task (run elevated without a service)

**What it looks like:**
- During setup, you create a Task Scheduler entry set to “Run with highest privileges.”
- UI triggers the task to run the privileged helper when needed, often without repeated UAC prompts.

**Pros**
- No always-running service.
- Can reduce prompts after initial setup.

**Cons**
- Still requires admin to create the task.
- Task Scheduler semantics are fiddly across Windows versions and enterprise policies.
- Can create surprising “why is this task running?” admin suspicion.
- Hard to make a clean, well-understood security boundary.

**When to consider**
- If you strongly want “no service” but also “no repeated prompts,” and you control the deployment environment.

---

### Option E — Kernel driver / minifilter

**Pros**
- Ultimate performance and integration.
- Can observe filesystem changes very directly.

**Cons**
- Highest risk: drivers expand kernel attack surface.
- Requires driver signing, more fragile across OS releases, higher support burden.
- Least user-friendly; most enterprise-heavy.

**Recommendation:** avoid unless you are building a security/EDR/forensics-grade product and you have the engineering and compliance budget.

---

### Option F — No-elevation mode (fallback)

This matters because a meaningful portion of your users:
- won’t have admin rights,
- won’t be able to install a service,
- or will decline UAC prompts.

Fallback strategies:
1) **Use standard Win32 enumeration** (`FindFirstFile`/`NtQueryDirectoryFile`) and accept slower performance.
2) **Leverage Windows Search index** where available (fast but only for indexed locations and has its own quirks). Windows Search’s architecture is designed around a background indexer and access restrictions.

**UX principle:** never dead-end the user. Always offer a functional (if slower) mode.

---

## 4) How other Windows tools handle this dilemma

### 4.1 Everything (voidtools)

Everything’s UX is the closest analog to your situation:
- It can read NTFS metadata for instant results.
- It explicitly offers two ways to handle privilege:
  - install a service so the UI runs standard, or
  - run the indexing process as administrator (portable mode).
- Users otherwise see prompts such as “requires administrative privileges to index NTFS volumes.”

Takeaway: **users accept a one-time install of a small helper service** far more readily than consenting to UAC every run.

### 4.2 Windows Search (Microsoft)

Windows Search runs an indexer as a service and maintains an index that is shared among users while applying security restrictions. Policies also reinforce that access restrictions still apply when indexing content.

Takeaway: the “privileged indexer + security-trimmed queries” architecture is a well-trodden Windows pattern.

### 4.3 Sysinternals-style utilities (general pattern)

Many admin-leaning tools:
- run in limited mode by default,
- offer “Run as Administrator” to unlock deeper features.

Takeaway: you can design a “basic mode” and a “power mode,” but the best consumer UX is still “install helper once.”

---

## 5) Recommended product design (“golden path”)

### 5.1 Default flows

**First launch (non-admin)**
- App starts instantly in standard mode.
- Banner: “Enable Instant Search (recommended)” with:
  - **Enable** (installs helper; triggers UAC once)
  - **Not now** (keeps standard mode)

**If user clicks Enable**
- Explain in one sentence what you’ll do:
  - “We’ll install a small local helper service that only reads file metadata to keep an index up to date. The search UI will continue to run without admin.”
- Trigger a single elevation to install/start service.
- Show indexing progress and “Ready” state.

**If user declines**
- Keep tool fully functional, but slower.
- Allow re-enabling later via Settings.

### 5.2 “Portable mode” (optional)

For a portable ZIP distribution:
- Offer “Run helper for this session”:
  - launches an elevated helper process,
  - UI stays standard,
  - user accepts UAC each time (but explicitly opted in).

### 5.3 Enterprise / managed endpoints

Provide two deployment-friendly options:
- MSI that installs the service (IT does it once).
- A mode that uses Windows Search or unprivileged enumeration (for environments where installing services is blocked).

---

## 6) Service architecture blueprint (practical details)

### 6.1 Component split

**UI (standard user)**
- Search box, filters, result view.
- No direct volume access.
- Talks to helper via IPC.

**Helper service (privileged)**
- Opens volumes and enumerates MFT/USN.
- Maintains an index database (per-machine or per-volume).
- Answers queries.

### 6.2 IPC choice

**Named pipes** are a common choice on Windows:
- fast,
- supports message or byte streams,
- supports access control and impersonation.

Microsoft documents:
- named pipe security descriptors and access rights,
- and that named pipes support impersonation in .NET.

### 6.3 Security trimming approaches (pick one, intentionally)

**Approach 1: Strict trimming (recommended default)**
- On query, helper impersonates client (or uses client identity) and only returns results the caller can access.
- Pros: prevents metadata leakage.
- Cons: Access checks can add overhead.

**Approach 2: “Everything-style” convenience mode (optional)**
- Return all file names; rely on “open will fail if you lack permission.”
- Pros: fastest, simplest.
- Cons: leaks names/paths; can be unacceptable in multi-user or regulated environments.

Practical compromise:
- Default to strict trimming.
- Provide a clearly labeled setting for advanced users (and default it off in enterprise builds).

### 6.4 Hardening tactics (high impact, low pain)

- Keep the service’s codebase tiny; no UI libraries.
- Validate all inputs (paths, query language).
- Rate-limit requests; protect against huge query payloads.
- Avoid loading arbitrary plugins in the service.
- Prefer read-only volume operations; do not include any write-capable IOCTLs.
- Log diagnostic events (optionally ETW) for supportability.

---

## 7) Recommendation matrix (what to choose)

### If your audience is “regular Windows users” (most user-friendly)
- Ship Option C as the default: **install helper service once**.
- Provide Option F fallback: unprivileged mode always works.

### If your audience is “portable tool / power users”
- Option B (elevated helper per-session) plus fallback.

### If your audience is “enterprise / managed endpoints”
- Option C, but plan for IT deployment and strict security trimming by default.

---

## 8) Suggested user-facing wording (copy you can steal)

**Banner:**  
“Enable Instant Search (recommended): install a small local helper service so search stays fast without running the app as administrator.”

**Details link:**  
“Why is this needed?” → “Windows protects low-level disk metadata access. The helper service reads file metadata to build an index; search results still respect access restrictions.”

**Buttons:**  
- “Enable Instant Search (requires admin once)”  
- “Continue in basic mode”

---

## 9) References (key sources)

- Microsoft: FSCTL_ENUM_USN_DATA enumerates USN data to obtain MFT records and uses a volume handle (`\\.\X:`):  
  https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data
- Microsoft: CreateFile3 requirements for opening a volume/disk include “caller must have administrative privileges”:  
  https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfile3
- Microsoft: UAC consent and elevation behavior (“must prompt the end user for consent”):  
  https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/how-it-works
- Microsoft: UAC architecture (Application Information service creates elevated processes):  
  https://learn.microsoft.com/en-us/windows/security/application-security/application-control/user-account-control/architecture
- Microsoft: Named pipe security descriptors:  
  https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
- Microsoft: Named pipes in .NET support impersonation:  
  https://learn.microsoft.com/en-us/dotnet/standard/io/pipe-operations
- voidtools (Everything): Everything service lets Everything run as standard user and is recommended for NTFS indexing on Vista+ for standard users:  
  https://www.voidtools.com/support/everything/everything_service/  
  https://www.voidtools.com/support/everything/options/  
  https://voidtools.com/forum/viewtopic.php?t=14335
- Windows Search “access restrictions still apply” (policy reference):  
  https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-search
- Windows Search described as a LocalSystem service with security restrictions on content access (overview reference):  
  https://www.windows-security.org/windows-service/windows-search
- Windows developer blog (Administrator Protection hardening context):  
  https://blogs.windows.com/windowsdeveloper/2025/05/19/enhance-your-application-security-with-administrator-protection/
