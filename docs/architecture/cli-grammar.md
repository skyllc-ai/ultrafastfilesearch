<!--
SPDX-FileCopyrightText: 2025-2026 SKY, LLC.
SPDX-License-Identifier: MPL-2.0
-->
# UFFS CLI Grammar — search-first, `--command` for everything else

**Status:** Design approved, implementation pending. This is the design +
implementation + tracking doc for the `uffs` command-line grammar redesign.

**TL;DR:** `uffs <anything>` searches for `<anything>` — *any* word, with no
reserved words. Management operations are `--<command>` (double-dash), e.g.
`uffs --update`, `uffs --daemon start`. The first token decides the mode:
a known `--command` → that command; anything else → search.

---

## 1. The problem — one token space, two meanings

UFFS is a **search-first** tool: the overwhelmingly common invocation is
`uffs <pattern>`. But today it also borrows the **subcommand-first** model for
management:

```
uffs update     # runs the self-updater   (NOT a search for "update")
uffs status     # system status           (NOT a search for "status")
uffs daemon …   # daemon control
uffs mcp …       uffs stats …   uffs agg …
```

Those six words (`update`, `status`, `daemon`, `mcp`, `stats`, `aggregate`/`agg`)
are **reserved first tokens** that *shadow* search patterns. Consequences:

1. **You cannot search for those bare words.** `uffs update` can never mean
   "find files named `update`". Globs (`uffs '*update*'`) still work; only the
   bare token is stolen.
2. **Cognitive burden.** The user must *memorize* which ~6 words are special —
   the antithesis of "search for anything".
3. **Wrong asymmetry.** The *common* case (search) loses bare words to the
   *rare* case (management). The cleanest syntax should belong to the common
   operation, not the rare one.

The collision is structural: a single token space (`uffs <X>`) cannot mean both
"data" (a pattern) and "instruction" (a command) without *either* reserving
words *or* separating the spaces. The only real question is **which space
management should use.**

## 2. Prior art — two schools

| School | Examples | First arg | Has this dilemma? |
|---|---|---|---|
| **Subcommand-first** | `git`, `cargo`, `docker`, `kubectl` | always a verb | No — there is no freeform primary mode |
| **Search-first** | `ripgrep`, `fd`, **Everything `es.exe`** | always the query | No — management is **flags**, positionals are pure |

UFFS is unambiguously **search-first** (its identity is `uffs <pattern>`), but it
adopted the subcommand-first *mechanism* — which is exactly why it collides. The
two cutting-edge tools in our space resolve it the same way:

- **`rg <pattern>`**, **`fd <pattern>`** — positional is *always* the query;
  every option is a flag; no bare-word subcommand can shadow a search.
- **Everything's own CLI (`es.exe`)** — `es <search>`; all options are `-flags`.
  Our direct competitor has **no** shadowing subcommands.

**Conclusion:** for a search-first tool, the positional space belongs to the
query, and management belongs in a *distinct* token space.

## 3. The grammar

### 3.1 The rule (the whole design in one sentence)

> **The first token decides the mode. If it is a known `--command`, run that
> command. Otherwise, it is a search.**

```
uffs update                         → search "update"          ✅ no longer shadowed
uffs -update                        → search "-update"         ✅ single dash stays a pattern
uffs '*.pdf' --sort -size           → search, sorted by size   ✅ search keeps its own flags
uffs --update                       → run the updater          ✅ -- = "addressing the tool"
uffs --update acquire --version v1  → updater, acquire action
uffs --daemon start                 → daemon, start action
```

### 3.2 The precision that makes it airtight: disjoint sets

Search **already uses `--` flags** (`--sort`, `--ext`, `--drive`, `--limit`,
`--format`, …). So the rule is **not** "any `--` is a command" — it is "**the
first token is a known *command* word**". The **command set** and the
**search-flag set** are deliberately **disjoint**, so there is never a clash:

```
uffs --ext pdf       → search (--ext is a search flag, not a command)   ✅
uffs --update        → updater (--update is in the command set)          ✅
uffs --sort -size    → search (--sort is a search flag)                  ✅
```

That disjointness is the trick: it lets a *pattern-less* search like
`uffs --ext pdf` coexist with `uffs --update`.

### 3.3 The mental model (one sentence the user learns once)

> **Bare or `-`-prefixed = data (a pattern). `--<verb>` = an instruction to the
> tool.**

It is mildly unconventional (most CLIs use `--` for *options*), but for a
search-first tool it is coherent and learnable, and it is the price of keeping
`uffs <anything>` literally mean "search for anything".

## 4. Uniform command model — every command, the same shape

```
uffs <pattern> [--search-options]          # search (the default; no command token)
uffs --<command> [<action>] [--<options>]  # management — UNIFORM across all commands
```

Rules applied to **every** command identically:

1. The command is `--<command>` (double-dash), only valid as the **first token**.
2. The **action** (if any) is a **bare positional** (no dash): `--daemon start`,
   `--update acquire`. (This normalizes today's inconsistency where `daemon`
   uses positional actions but `update` uses `--acquire`/`--apply` flags.)
3. **Options** are `--flags` after the action: `--update acquire --version v1`.
4. This mirrors `git`'s `command → action → --option` shape, applied uniformly.

**Standalone binaries keep conventional flags.** `uffs-broker --install`,
`uffs-broker --start`, `uffsd --no-retire` — these are *separate programs* with
their own argv parsers, not the `uffs` umbrella, so they follow the normal
"`--` = option" convention. There is no inconsistency: different entry points,
different (each internally-consistent) conventions.

## 5. Complete command surface (old → new)

| Today (shadowing) | New (uniform) | Actions | Key options |
|---|---|---|---|
| `uffs <pattern>` | `uffs <pattern>` *(unchanged)* — also explicit `uffs --search <pattern>` | — | `--sort --ext --drive --limit --format …` |
| `uffs stats [path]` | `uffs --stats [path]` | — | `--top N` `--data-dir` `--mft-file` |
| `uffs aggregate\|agg <preset>` | `uffs --agg <preset>` | — | `--format` |
| `uffs daemon <a>` | `uffs --daemon <a>` | `start` `stop` `restart` `status` | `--data-dir` `--mft-file` `--elevate` |
| `uffs mcp <a>` | `uffs --mcp <a>` | `run` `serve` `stop` `status` | `--bind` `--port` `--data-dir` |
| `uffs update [--acquire\|--apply\|--snapshot]` + `uffs update doctor` | `uffs --update [<a>]` | *(none=detect)* `snapshot` `acquire` `apply` `doctor` `recover` | `--version` `--repair` `--offline` `--repo` |
| `uffs status` | `uffs --status` | — | — |
| `uffs --help / --version` | `uffs --help / --version` *(unchanged; global)* | — | — |

**Command set (the disambiguator):**
`--search`, `--stats`, `--agg` (alias `--aggregate`), `--daemon`, `--mcp`,
`--update`, `--status`. (`--help`/`--version` are global, handled first.)

`doctor` is an **action of `--update`** (`uffs --update doctor`) — it is part of
the update subsystem, so this keeps the surface uniform. (A top-level `--doctor`
convenience alias is an open question, §12.)

## 6. Edge cases & escapes

| Input | Result |
|---|---|
| `uffs` (no args) | help (unchanged) |
| `uffs --ext pdf` | search, all `.pdf` (pattern-less search; `--ext` ∉ command set) |
| `uffs -- --update` | search for the literal pattern `--update` (bare `--` = end-of-options) |
| `uffs --search -- --update` | same, explicit search form |
| `uffs --update --help` | update help (the `--help` after a command is command-scoped) |
| `uffs --bogus` | error: unknown command `--bogus` (a `--`-leading first token that is *not* a command + *not* a search flag is a usage error, with a "did you mean …?" hint) |

The **only** thing not searchable bare is a filename literally beginning with
`--` (e.g. `--update`), reachable with `uffs -- <pattern>`. Such filenames are
pathological; the escape is the universal `--` separator.

## 7. Why not the alternatives

- **Keep bare-word subcommands** (status quo): shadows ~6 common words; forces
  memorization; wrong asymmetry. Rejected.
- **Plain flags for management, `rg`-style** (`uffs --update`, but treat *any*
  leading `-`/`--` as non-pattern): still taxes **single-dash** patterns
  (`-update` would need escaping). The `--`-only rule (this design) frees
  single-dash patterns — strictly better here.
- **A sigil** (`uffs :update`, `uffs @update`): keeps positionals pure, but
  sigils are less discoverable than `--`, and UFFS already overloads `>` for
  regex — adding another sigil muddies the model. Rejected.

## 8. Implementation plan

All top-level dispatch lives in `crates/uffs-cli/src/main.rs::run()`. Search
argument parsing stays in `commands::search` / `SearchParams::from_cli_args`.

### 8.1 The dispatcher (the core change)

In `run()`:

```rust
// after the global --help/--version fast path + maybe_self_heal()
let first = tokens.first().copied().unwrap_or("");

// Bare `--` separator → force search of the remaining tokens.
if first == "--" {
    return run_search(raw_args.get(2..).unwrap_or_default());
}

match Command::from_token(first) {
    Some(cmd) => dispatch_command(cmd, raw_args.get(2..).unwrap_or_default()),
    None      => run_search(raw_args.get(1..).unwrap_or_default()), // default = search
}
```

```rust
/// The management command set — the ONLY tokens that switch out of search mode.
/// Deliberately DISJOINT from every search flag name (`--sort`, `--ext`, …).
enum Command { Search, Stats, Agg, Daemon, Mcp, Update, Status }

impl Command {
    fn from_token(tok: &str) -> Option<Self> {
        Some(match tok {
            "--search"               => Self::Search,
            "--stats"                => Self::Stats,
            "--agg" | "--aggregate"  => Self::Agg,
            "--daemon"               => Self::Daemon,
            "--mcp"                  => Self::Mcp,
            "--update"               => Self::Update,
            "--status"               => Self::Status,
            _ => return None,
        })
    }
}
```

A **debug-assert / unit test** enforces the disjointness invariant: no
`Command` token may equal any known search-flag long name.

### 8.2 Per-command handlers (mostly re-wiring existing code)

- `run_search` — unchanged; now also the `--search` target.
- `run_stats` — read the optional `[path]` positional + `--top`.
- `run_aggregate` — read the `<preset>` positional.
- `run_daemon` — already action-style (`start`/`stop`/…); unchanged internally.
- `commands::mcp_mgmt` — already action-style; unchanged internally.
- `commands::update::run_update` — **normalize**: today it reads `--acquire` /
  `--apply` *flags* + a `doctor` token; change to read a leading **action**
  positional (`snapshot`/`acquire`/`apply`/`doctor`/`recover`, none = detect),
  with `--version`/`--repair`/`--offline`/`--repo` as options.
- `system_status` — unchanged.

### 8.3 `--<command> --help`

Each command handler checks for `--help`/`-h` in its args and prints a
command-scoped help (the daemon/status handlers already do this). `print_help`
gains the new top-level grammar.

### 8.4 Removals (no back-compat — pre-1.0)

There is **no external API/version-stability promise** (pre-1.0; binaries are
the product, not a stable CLI contract), so the bare-word subcommands are
**removed**, not aliased. `uffs update` now *searches* for "update". No
deprecation shim is shipped. (If field reports later show muscle-memory pain, a
one-release `::warning::`-style nudge can be reconsidered — but it is explicitly
out of scope here.)

### 8.5 Internal callers

`run_stats`/`run_aggregate` synthesize search args and call `run_search` — keep
that internal path; only the *external* entry tokens change.

## 9. Testing strategy

1. **Dispatcher unit tests** (pure, table-driven):
   - `update` (bare) → Search mode, pattern == "update".
   - `-update` → Search, pattern == "-update".
   - `--update` → Update command.
   - `--ext` (first token) → Search mode (search flag, not a command).
   - `--` then `--update` → Search, pattern == "--update".
   - `--bogus` → usage error.
2. **Disjointness invariant test**: assert no `Command` token collides with any
   search-flag long name (fails loudly if someone adds `--sort` as a command).
3. **Per-command parse tests**: `--update acquire --version v1` → action=acquire,
   version=v1; `--daemon start` → action=start; etc.
4. **Golden help-text test**: the new grammar renders + lists every command.
5. **Regression**: every existing search test still passes through the default
   path unchanged.

## 10. Surfaces to update (docs + help)

- `crates/uffs-cli/src/args.rs` — `print_help`, plus per-command help texts.
- `crates/uffs-cli/src/commands/update/mod.rs` — `print_help` for the updater.
- `CLAUDE.md` — the command examples.
- `README.md` — any `uffs <subcommand>` examples.
- MCP server instructions / `uffs-mcp` docs — if they reference CLI subcommands.
- This doc — flip **Status** to "Implemented" + check off §11.

## 11. Tracking checklist

- [ ] **P0 — Dispatcher.** `Command` enum + `from_token` + `run()` rewrite +
      bare-`--` escape. Dispatcher unit tests + disjointness invariant test.
- [ ] **P1 — Normalize `--update`.** Action-positional parsing
      (snapshot/acquire/apply/doctor/recover); options as flags. Update its
      `print_help`. Tests.
- [ ] **P2 — Wire the rest.** `--stats`, `--agg`, `--daemon`, `--mcp`,
      `--status`, `--search`. Per-command help (`--<cmd> --help`).
- [ ] **P3 — Top-level help + usage errors.** New `print_help`; "unknown command
      `--x` (did you mean …?)"; golden help test.
- [ ] **P4 — Docs.** CLAUDE.md, README, MCP instructions, this doc → Implemented.
- [ ] **P5 — Validate.** Host + Windows-MSVC clippy clean; full nextest; manual
      smoke of every command + a `uffs update` *search*.

## 12. Open questions / decisions

1. **Top-level `--doctor`?** Keep `uffs --update doctor` only (uniform), or add
   a `--doctor` convenience alias? *Lean: keep it an update action; revisit if
   users reach for `uffs --doctor`.*
2. **`--search` explicit form** — ship it (uniformity + a clean scripting form)
   or leave search bare-only? *Lean: ship `--search` as the explicit twin of the
   bare default.*
3. **Short command aliases?** e.g. `-u` for `--update`. *Lean: no — short single
   dashes are reserved for patterns/search-short-flags; keep commands `--long`
   only to preserve the "single dash = data" rule.*
