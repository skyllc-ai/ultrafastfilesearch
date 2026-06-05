#!/usr/bin/env rust-script
//! ```cargo
//! [dependencies]
//! anyhow = "1.0"
//! clap = { version = "4.0", features = ["derive"] }
//! colored = "2.0"
//! serde_json = "1.0"
//! ```
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.
//
// =============================================================================
// scripts/windows/create-corrupted-name-tree.rs
//   — the "can a malicious actor hide a file behind a crooked name?" torture tree
// =============================================================================
//
// Builds a directory full of files and subdirectories whose names span the full
// spectrum of "hostile" NTFS naming: unpaired UTF-16 surrogates (legal on disk,
// ILLEGAL in UTF-8 — the WI-4.4 target), extreme CJK / combining-mark / RTL /
// zero-width / astral-emoji names, control characters, trailing dots/spaces,
// reserved DOS device stems, and very long names. EVERY name embeds the same
// searchable marker token (default `UFFSZZQ`) so the whole set is recoverable
// in one query:
//
//     uffs search UFFSZZQ --drives G
//
// **The point.** UFFS's WI-4.4 work claims a malicious actor cannot hide a file
// from UFFS behind an ill-formed name. This script manufactures exactly those
// files so you can PROVE it: count how many you created, then count how many
// UFFS finds by the common marker. They must match. If a surrogate-named file
// is missing from the UFFS result, it was hidden — and that is the bug WI-4.4
// exists to prevent.
//
// **Why a raw Win32 path for the corrupted names.** Rust's `std::fs` ultimately
// hands the OS a UTF-16 path, but the public API channels names through `Path`/
// `OsStr`. To place a *lone surrogate* on disk we build a `Vec<u16>` directly
// and call `CreateFileW` / `CreateDirectoryW` with a `\\?\`-prefixed path (the
// prefix disables Win32 path normalisation, which would otherwise reject
// trailing dots/spaces and reserved stems — the very names we want to test).
//
// **Windows only, and it WRITES TO DISK.** It creates a single top-level folder
// (default `UFFS_corrupted_names`) under the target drive root and populates it.
// Nothing outside that folder is touched. `--cleanup` removes the whole folder.
// Re-running is idempotent (existing entries are skipped / overwritten).
//
// Defaults: --drive G   --marker UFFSZZQ   --root-dir UFFS_corrupted_names
// (so the tree lands at  G:\UFFS_corrupted_names\ ).
//
// Usage:
//   rust-script scripts/windows/create-corrupted-name-tree.rs                 # G: drive, default marker
//   rust-script scripts/windows/create-corrupted-name-tree.rs --drive D
//   rust-script scripts/windows/create-corrupted-name-tree.rs --marker MYTAG7
//   rust-script scripts/windows/create-corrupted-name-tree.rs --root-dir crooked --cleanup
//   rust-script scripts/windows/create-corrupted-name-tree.rs --dry-run        # print plan, write nothing
//   rust-script scripts/windows/create-corrupted-name-tree.rs --list           # show TRUE on-disk names (no UFFS needed)
//   rust-script scripts/windows/create-corrupted-name-tree.rs --verify          # run uffs + auto-compare (the WI-4.4 proof)
//   rust-script scripts/windows/create-corrupted-name-tree.rs --verify --bin C:\Users\me\bin\uffs.exe
//
// After it creates the tree it prints the exact verification command and count.
//
// **Seeing the files WITHOUT UFFS.** `--list` enumerates the real folder via
// `FindFirstFileW` and prints each true name as a lossy display string PLUS its
// raw UTF-16 code units (hex), flagging any name with an unpaired surrogate.
// Explorer and `dir` mangle or silently drop these names; `--list` shows what
// the kernel actually stored. (`--dry-run` shows the PLAN before creating;
// `--list` shows REALITY after.)
//
// **Proving it WITH UFFS, automatically.** `--verify` enumerates the on-disk
// entries (ground truth), then runs two cross-checks and exits non-zero on any
// failure:
//   1. `uffs search <marker> --filter all` — every on-disk name MUST appear
//      (nothing is hidden behind a crooked name).
//   2. `uffs search * --malformed` — must return EXACTLY the ill-formed
//      entries on disk (the forensic malformed-name filter finds the crooked
//      names and only those).
// This is the WI-4.4 findability claim AND the --malformed filter, checked in
// one command.

#![allow(clippy::print_stdout, clippy::print_stderr)] // a CLI tool: stdout IS the UI.

use anyhow::{Context as _, Result};
use clap::Parser;
use colored::Colorize as _;

/// The searchable marker is wrapped so it forms a recognizable substring even
/// when surrounded by hostile codepoints. Kept ASCII so it is queryable by a
/// normal substring search regardless of the surrounding corruption.
const DEFAULT_MARKER: &str = "UFFSZZQ";

/// Default drive letter (no colon, no slash) — the user's box maps the big
/// test corpus to `G:`.
const DEFAULT_DRIVE: &str = "G";

/// Default top-level folder created under `<drive>:\`.
const DEFAULT_ROOT_DIR: &str = "UFFS_corrupted_names";

#[derive(Parser, Debug)]
#[command(
    name = "create-corrupted-name-tree",
    about = "Create files/dirs with corrupted & extreme-Unicode names sharing a common marker, for UFFS findability testing."
)]
struct Cli {
    /// Target drive letter (e.g. `G`, `D`). Colon/slashes are stripped if present.
    #[arg(long, default_value = DEFAULT_DRIVE)]
    drive: String,

    /// Top-level folder name created under `<drive>:\`.
    #[arg(long, default_value = DEFAULT_ROOT_DIR)]
    root_dir: String,

    /// The ASCII marker token embedded in EVERY generated name.
    #[arg(long, default_value = DEFAULT_MARKER)]
    marker: String,

    /// Remove the whole top-level folder and exit (does not create anything).
    #[arg(long)]
    cleanup: bool,

    /// Print exactly what would be created (with byte-level detail) and write nothing.
    #[arg(long)]
    dry_run: bool,

    /// List the entries ACTUALLY on disk in the target folder (true names as
    /// display + raw UTF-16 hex) and exit. This is how you "see" the corrupted
    /// files on a box without UFFS — Explorer/`dir` mangle or hide them.
    #[arg(long)]
    list: bool,

    /// Run `uffs search <marker>` and compare what UFFS finds against what is
    /// actually on disk. Exits non-zero if UFFS hides any on-disk entry — the
    /// WI-4.4 findability claim, checked automatically. Implies the tree already
    /// exists (run without flags first to create it).
    #[arg(long)]
    verify: bool,

    /// Path to the `uffs` binary used by `--verify`. Defaults to
    /// `%USERPROFILE%\bin\uffs.exe`, then `target\release\uffs.exe`, then bare
    /// `uffs.exe` on PATH.
    #[arg(long)]
    bin: Option<String>,
}

/// One planned entry: its UTF-16 name (relative to the root folder) and whether
/// it is a directory. The name is `Vec<u16>` so it can carry lone surrogates
/// that no `String`/`OsString`-from-`&str` could represent.
struct PlannedEntry {
    /// UTF-16 code units of the entry's leaf name (no path separators).
    name_utf16: Vec<u16>,
    /// Human-readable description of WHY this name is hostile (for the report).
    why: &'static str,
    /// `true` → create a directory; `false` → create an (empty) file.
    is_dir: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{} {err:#}", "error:".red().bold());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    #[cfg(not(windows))]
    {
        let _ = &cli;
        anyhow::bail!(
            "this tool creates NTFS names with unpaired surrogates and reserved stems via the \
             Win32 wide API; it only runs on Windows. (You can still read the source to see what \
             it would create.)"
        );
    }

    #[cfg(windows)]
    {
        let drive = normalize_drive(&cli.drive)?;
        let root = format!(r"{drive}:\{}", cli.root_dir);
        // The `\\?\` long-path / no-normalisation prefix is what lets us place
        // names with trailing dots, trailing spaces and reserved stems.
        let root_win = format!(r"\\?\{root}");

        if cli.cleanup {
            return windows_impl::cleanup(&root, &root_win);
        }

        if cli.list {
            return windows_impl::list_on_disk(&root, &root_win);
        }

        let marker = cli.marker.trim();
        anyhow::ensure!(!marker.is_empty(), "--marker must not be empty");
        anyhow::ensure!(
            marker.chars().all(|c| c.is_ascii_alphanumeric()),
            "--marker should be ASCII-alphanumeric so it stays a clean searchable substring (got {marker:?})"
        );

        if cli.verify {
            return windows_impl::verify(&root, &root_win, drive, marker, cli.bin.as_deref());
        }

        let plan = build_plan(marker);

        print_header(&root, marker, plan.len());

        if cli.dry_run {
            windows_impl::report_plan(&plan, /*created=*/ false);
            println!(
                "\n{} dry run — nothing written. Re-run without {} to create the tree.",
                "note:".yellow().bold(),
                "--dry-run".cyan()
            );
            return Ok(());
        }

        let created = windows_impl::materialize(&root, &root_win, &plan)?;
        windows_impl::report_plan(&plan, /*created=*/ true);
        print_footer(&drive, marker, created, plan.len());
        Ok(())
    }
}

/// Strip a trailing `:` / slashes and validate a single-letter drive.
fn normalize_drive(raw: &str) -> Result<char> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_uppercase();
    let mut chars = cleaned.chars();
    let letter = chars
        .next()
        .context("--drive must contain a drive letter (e.g. G)")?;
    anyhow::ensure!(
        chars.next().is_none(),
        "--drive must be a single letter (got {raw:?})"
    );
    Ok(letter)
}

/// Build the full set of hostile names, each embedding `marker`. This is the
/// platform-agnostic part (it just assembles `Vec<u16>` names + metadata); the
/// Windows-only `materialize` turns them into real on-disk entries.
fn build_plan(marker: &str) -> Vec<PlannedEntry> {
    let m: Vec<u16> = marker.encode_utf16().collect();
    let mut plan: Vec<PlannedEntry> = Vec::new();

    // Helper: assemble a name from interleaved UTF-16 chunks.
    let join = |parts: &[&[u16]]| -> Vec<u16> {
        let mut v = Vec::new();
        for p in parts {
            v.extend_from_slice(p);
        }
        v
    };
    let ascii = |s: &str| -> Vec<u16> { s.encode_utf16().collect() };

    // ── 1. Unpaired surrogates — THE WI-4.4 target (illegal in UTF-8) ───────
    // A lone HIGH surrogate (0xD800). Legal on NTFS, no UTF-8 representation.
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_loneHigh_"), &[0xD800], &ascii(".txt")]),
        why: "unpaired HIGH surrogate U+D800 (WI-4.4 target — no UTF-8 form)",
        is_dir: false,
    });
    // A lone LOW surrogate (0xDC00).
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_loneLow_"), &[0xDC00], &ascii(".txt")]),
        why: "unpaired LOW surrogate U+DC00 (WI-4.4 target — no UTF-8 form)",
        is_dir: false,
    });
    // High surrogate followed by a NON-low BMP char → still unpaired.
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_highThenAscii_"), &[0xD834, 0x0041], &ascii(".bin")]),
        why: "high surrogate U+D834 followed by 'A' (not a low surrogate → unpaired)",
        is_dir: false,
    });
    // Two high surrogates in a row.
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_twoHighs_"), &[0xD800, 0xD801], &ascii(".dat")]),
        why: "two consecutive HIGH surrogates (both unpaired)",
        is_dir: false,
    });
    // Reversed pair: low then high (never a valid pair in that order).
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_reversedPair_"), &[0xDC00, 0xD800], &ascii(".dat")]),
        why: "low-then-high surrogates (reversed — never a valid pair)",
        is_dir: false,
    });
    // A surrogate-named DIRECTORY (dirs walk the parent chain differently).
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_dirLoneHigh_"), &[0xD800]]),
        why: "DIRECTORY with an unpaired HIGH surrogate (path-resolver torture)",
        is_dir: true,
    });

    // ── 2. Valid astral pairs (sanity: these MUST become 4-byte UTF-8) ──────
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_emoji_"), &[0xD83D, 0xDE00], &ascii(".txt")]),
        why: "valid surrogate pair 😀 U+1F600 (control — must be normal 4-byte UTF-8)",
        is_dir: false,
    });
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_astralCJK_"), &[0xD869, 0xDED6], &ascii(".txt")]),
        why: "astral CJK 𪛖 U+2A6D6 (valid pair — extension-plane Han)",
        is_dir: false,
    });

    // ── 3. Extreme but VALID Unicode: CJK / Hangul / Kana / combining / RTL ─
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_zh_"), &ascii("文件名测试報告"), &ascii(".txt")]),
        why: "Chinese (Simplified + Traditional Han): 文件名测试報告",
        is_dir: false,
    });
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_ja_"), &ascii("日本語のファイル名テスト"), &ascii(".txt")]),
        why: "Japanese (Kanji + Hiragana + Katakana): 日本語のファイル名テスト",
        is_dir: false,
    });
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_ko_"), &ascii("한국어파일이름테스트"), &ascii(".txt")]),
        why: "Korean (Hangul): 한국어파일이름테스트",
        is_dir: false,
    });
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_ar_"), &ascii("اختبار_الملف"), &ascii(".txt")]),
        why: "Arabic (RTL script): اختبار_الملف",
        is_dir: false,
    });
    plan.push(PlannedEntry {
        name_utf16: join(&[&m, &ascii("_he_"), &ascii("בדיקת_קובץ"), &ascii(".txt")]),
        why: "Hebrew (RTL script): בדיקת_קובץ",
        is_dir: false,
    });
    // Combining marks (NFD): base + combining acute accents stacked.
    plan.push(PlannedEntry {
        name_utf16: join(&[
            &m,
            &ascii("_combining_e"),
            &[0x0301, 0x0301, 0x0301], // three stacked combining acute accents
            &ascii(".txt"),
        ]),
        why: "decomposed combining marks (e + 3× U+0301 combining acute)",
        is_dir: false,
    });
    // Right-to-left override control char embedded mid-name (display spoofing).
    plan.push(PlannedEntry {
        name_utf16: join(&[
            &m,
            &ascii("_rlo_invoice"),
            &[0x202E], // RIGHT-TO-LEFT OVERRIDE
            &ascii("fdp.exe"), // renders as "exe.pdf" — classic spoof
        ]),
        why: "RIGHT-TO-LEFT OVERRIDE U+202E (extension-spoofing display attack)",
        is_dir: false,
    });
    // Zero-width characters (invisible) padding the name.
    plan.push(PlannedEntry {
        name_utf16: join(&[
            &m,
            &ascii("_zerowidth"),
            &[0x200B, 0xFEFF, 0x200D], // ZWSP, ZWNBSP/BOM, ZWJ
            &ascii(".txt"),
        ]),
        why: "zero-width chars (U+200B ZWSP, U+FEFF, U+200D ZWJ — invisible)",
        is_dir: false,
    });

    // ── 4. Control characters in the name (legal on NTFS, hostile to display) ─
    // Note: 0x00 (NUL) and path separators (/ \ : * ? " < > |) cannot be in a
    // name even via \\?\; we use the LEGAL-but-nasty C0 controls 0x01..0x1F.
    plan.push(PlannedEntry {
        name_utf16: join(&[
            &m,
            &ascii("_ctrl_"),
            &[0x0001, 0x0007, 0x001B], // SOH, BEL, ESC
            &ascii(".txt"),
        ]),
        why: "C0 control chars (U+0001 SOH, U+0007 BEL, U+001B ESC)",
        is_dir: false,
    });

    // ── 5. Structural edge cases (\\?\ defeats Win32 normalisation) ─────────
    // Trailing dot — Win32 would strip it; NTFS keeps it.
    plan.push(PlannedEntry {
        name_utf16: ascii(&format!("{marker}_trailingdot...")),
        why: "trailing dots (Win32 strips; NTFS retains)",
        is_dir: false,
    });
    // Trailing space — same story.
    plan.push(PlannedEntry {
        name_utf16: ascii(&format!("{marker}_trailingspace   ")),
        why: "trailing spaces (Win32 strips; NTFS retains)",
        is_dir: false,
    });
    // Reserved DOS device stem (CON) — Win32 refuses; \\?\ allows on NTFS.
    plan.push(PlannedEntry {
        name_utf16: ascii(&format!("{marker}_CON.txt")),
        why: "reserved DOS device stem 'CON' (Win32 refuses; \\\\?\\ allows)",
        is_dir: false,
    });
    plan.push(PlannedEntry {
        name_utf16: ascii(&format!("{marker}_NUL.dat")),
        why: "reserved DOS device stem 'NUL'",
        is_dir: false,
    });
    // Leading/embedded dots.
    plan.push(PlannedEntry {
        name_utf16: ascii(&format!(".{marker}.hidden.dotfile")),
        why: "leading dot + multiple embedded dots",
        is_dir: false,
    });
    // Very long single-segment name (well under MAX_PATH thanks to \\?\, but long).
    plan.push(PlannedEntry {
        name_utf16: ascii(&format!("{marker}_{}.txt", "x".repeat(200))),
        why: "very long name (~210 chars; valid only via \\\\?\\ long-path prefix)",
        is_dir: false,
    });
    // All-emoji-ish + marker, mixed scripts in one name (the kitchen sink).
    plan.push(PlannedEntry {
        name_utf16: join(&[
            &m,
            &ascii("_mixed_"),
            &ascii("文"),
            &[0xD83D, 0xDCC1], // 📁 valid pair
            &ascii("名"),
            &[0xD800], // unpaired high surrogate — the sting in the tail
            &ascii(".txt"),
        ]),
        why: "kitchen sink: Han + valid emoji + unpaired surrogate in one name",
        is_dir: true,
    });

    plan
}

/// Print the run header (shown for both dry-run and real runs).
fn print_header(root: &str, marker: &str, count: usize) {
    println!(
        "{}",
        "── UFFS corrupted-name torture tree ─────────────────────────".bold()
    );
    println!("  target folder : {}", root.cyan());
    println!("  common marker : {}", marker.green().bold());
    println!("  entries       : {count}");
    println!();
}

/// Print the closing summary + the exact verification commands.
#[cfg(windows)]
fn print_footer(drive: &char, marker: &str, created: usize, planned: usize) {
    println!();
    if created == planned {
        println!(
            "{} created all {created} entries.",
            "success:".green().bold()
        );
    } else {
        println!(
            "{} created {created} of {planned} entries. The rest were rejected by NTFS \
             itself on this volume (e.g. names with C0 control characters) — that is the \
             filesystem's limit, not a tool error, and is expected on some volumes. The \
             {created} that landed are the corpus to verify.",
            "partial:".yellow().bold()
        );
    }
    println!();
    println!("{}", "── Verify ───────────────────────────────────────────────────".bold());
    println!(
        "  Auto (recommended): {}",
        format!("rust-script {} --drive {drive} --verify", file_stem_self()).cyan()
    );
    println!("      runs uffs, cross-references on-disk vs found, PASS/FAIL + exit code.");
    println!();
    println!(
        "  Manual: {}",
        format!("uffs search {marker} --drives {drive} --filter all --limit 200").cyan()
    );
    println!(
        "      expected: {} results (every created name embeds the marker). If a \
         surrogate-named entry is MISSING, a file was hidden — the WI-4.4 bug.",
        created.to_string().green().bold()
    );
    println!();
    println!(
        "  See true on-disk names (no UFFS): {}",
        format!("rust-script {} --drive {drive} --list", file_stem_self()).cyan()
    );
    println!(
        "  Cleanup: {}",
        format!("rust-script {} --drive {drive} --cleanup", file_stem_self()).cyan()
    );
}

/// Best-effort self-path for the printed cleanup hint.
#[cfg(windows)]
fn file_stem_self() -> String {
    std::env::args()
        .next()
        .unwrap_or_else(|| "scripts/windows/create-corrupted-name-tree.rs".to_owned())
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows-only filesystem materialisation via the wide Win32 API.
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(windows)]
mod windows_impl {
    use super::PlannedEntry;
    use anyhow::{Context as _, Result};
    use colored::Colorize as _;
    use std::os::windows::ffi::OsStrExt as _;

    // Minimal Win32 FFI. We deliberately avoid pulling in the `windows` crate
    // so this stays a single-file rust-script with tiny deps.
    #[allow(non_snake_case)]
    mod ffi {
        use core::ffi::c_void;
        pub type Handle = *mut c_void;
        pub const INVALID_HANDLE_VALUE: Handle = !0_isize as Handle;
        pub const GENERIC_WRITE: u32 = 0x4000_0000;
        pub const CREATE_ALWAYS: u32 = 2;
        pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
        pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
        // Errors we treat as "already exists" (idempotent re-run).
        pub const ERROR_ALREADY_EXISTS: u32 = 183;
        pub const ERROR_FILE_EXISTS: u32 = 80;
        // `FindFirstFileW` returning "no more files" on an empty dir.
        pub const ERROR_NO_MORE_FILES: u32 = 18;

        /// Layout-compatible with the Win32 `WIN32_FIND_DATAW`. We only read
        /// `dwFileAttributes` and `cFileName`; the rest are sized placeholders
        /// so the struct matches the ABI the kernel writes.
        #[repr(C)]
        pub struct Win32FindDataW {
            pub dwFileAttributes: u32,
            pub ftCreationTime: [u32; 2],
            pub ftLastAccessTime: [u32; 2],
            pub ftLastWriteTime: [u32; 2],
            pub nFileSizeHigh: u32,
            pub nFileSizeLow: u32,
            pub dwReserved0: u32,
            pub dwReserved1: u32,
            /// MAX_PATH UTF-16 code units, NUL-terminated.
            pub cFileName: [u16; 260],
            pub cAlternateFileName: [u16; 14],
        }

        #[link(name = "kernel32")]
        extern "system" {
            pub fn CreateFileW(
                lpFileName: *const u16,
                dwDesiredAccess: u32,
                dwShareMode: u32,
                lpSecurityAttributes: *mut c_void,
                dwCreationDisposition: u32,
                dwFlagsAndAttributes: u32,
                hTemplateFile: Handle,
            ) -> Handle;
            pub fn CreateDirectoryW(
                lpPathName: *const u16,
                lpSecurityAttributes: *mut c_void,
            ) -> i32;
            pub fn CloseHandle(hObject: Handle) -> i32;
            pub fn GetLastError() -> u32;
            pub fn FindFirstFileW(
                lpFileName: *const u16,
                lpFindFileData: *mut Win32FindDataW,
            ) -> Handle;
            pub fn FindNextFileW(hFindFile: Handle, lpFindFileData: *mut Win32FindDataW) -> i32;
            pub fn FindClose(hFindFile: Handle) -> i32;
        }
    }

    /// NUL-terminate a UTF-16 path for the wide API.
    fn wide_z(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = std::ffi::OsStr::new(s).encode_wide().collect();
        v.push(0);
        v
    }

    /// Build a NUL-terminated `\\?\drive:\root\<leaf>` path where `<leaf>` is a
    /// raw UTF-16 buffer that may contain lone surrogates. We assemble the
    /// prefix from a normal `&str` and then splice the raw leaf units in, so the
    /// surrogate units survive verbatim (an intermediate `String`/`OsString`
    /// built from `&str` could never hold them anyway).
    fn wide_child_z(root_win: &str, leaf_utf16: &[u16]) -> Vec<u16> {
        let mut v: Vec<u16> = std::ffi::OsStr::new(root_win).encode_wide().collect();
        v.push(u16::from(b'\\'));
        v.extend_from_slice(leaf_utf16);
        v.push(0);
        v
    }

    /// Create the root folder (normal path — its name is ASCII) and then every
    /// planned child. Returns the count successfully created (or already
    /// present). Names NTFS itself rejects are reported and skipped, not fatal.
    pub fn materialize(root: &str, root_win: &str, plan: &[PlannedEntry]) -> Result<usize> {
        create_dir_str(root_win)
            .with_context(|| format!("creating root folder {root}"))?;

        let mut created = 0_usize;
        for entry in plan {
            let path_z = wide_child_z(root_win, &entry.name_utf16);
            let ok = if entry.is_dir {
                create_dir_wide(&path_z)
            } else {
                create_file_wide(&path_z)
            };
            match ok {
                Ok(()) => created += 1,
                Err(code) => {
                    eprintln!(
                        "  {} NTFS rejected an entry (GetLastError={code}): {}",
                        "skip:".yellow(),
                        entry.why
                    );
                }
            }
        }
        Ok(created)
    }

    /// Create a directory from a normal `&str` path (used for the root).
    fn create_dir_str(path_win: &str) -> Result<()> {
        let z = wide_z(path_win);
        // SAFETY: `z` is a valid NUL-terminated wide string for the call's lifetime.
        let rc = unsafe { ffi::CreateDirectoryW(z.as_ptr(), core::ptr::null_mut()) };
        if rc != 0 {
            return Ok(());
        }
        // SAFETY: immediately after a failed Win32 call.
        let err = unsafe { ffi::GetLastError() };
        if err == ffi::ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        anyhow::bail!("CreateDirectoryW failed (GetLastError={err})");
    }

    /// Create a directory from a raw wide path; `Err(code)` carries GetLastError.
    fn create_dir_wide(path_z: &[u16]) -> std::result::Result<(), u32> {
        // SAFETY: `path_z` is NUL-terminated and lives for the call.
        let rc = unsafe { ffi::CreateDirectoryW(path_z.as_ptr(), core::ptr::null_mut()) };
        if rc != 0 {
            return Ok(());
        }
        // SAFETY: immediately after a failed Win32 call.
        let err = unsafe { ffi::GetLastError() };
        if err == ffi::ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        Err(err)
    }

    /// Create an empty file from a raw wide path; `Err(code)` carries GetLastError.
    fn create_file_wide(path_z: &[u16]) -> std::result::Result<(), u32> {
        // SAFETY: `path_z` is NUL-terminated and lives for the call; the handle
        // is closed on the success path below.
        let handle = unsafe {
            ffi::CreateFileW(
                path_z.as_ptr(),
                ffi::GENERIC_WRITE,
                0,
                core::ptr::null_mut(),
                ffi::CREATE_ALWAYS,
                ffi::FILE_ATTRIBUTE_NORMAL,
                core::ptr::null_mut(),
            )
        };
        if handle == ffi::INVALID_HANDLE_VALUE {
            // SAFETY: immediately after a failed Win32 call.
            let err = unsafe { ffi::GetLastError() };
            if err == ffi::ERROR_FILE_EXISTS || err == ffi::ERROR_ALREADY_EXISTS {
                return Ok(());
            }
            return Err(err);
        }
        // SAFETY: `handle` is a valid handle returned by CreateFileW.
        unsafe { ffi::CloseHandle(handle) };
        Ok(())
    }

    /// Print the plan as a numbered list with the WTF-8 byte preview for each
    /// name (so corrupted names are visible even though the terminal can't
    /// render them).
    pub fn report_plan(plan: &[PlannedEntry], created: bool) {
        let verb = if created { "Created" } else { "Would create" };
        println!("{verb} {} entries:", plan.len());
        for (i, e) in plan.iter().enumerate() {
            let kind = if e.is_dir { "dir " } else { "file" };
            // Show the lossy display form + the raw UTF-16 units (hex) so the
            // exact bytes are auditable regardless of terminal capability.
            let display = String::from_utf16_lossy(&e.name_utf16);
            let units: Vec<String> =
                e.name_utf16.iter().map(|u| format!("{u:04X}")).collect();
            println!(
                "  {:>2}. [{}] {}",
                i + 1,
                kind.magenta(),
                e.why
            );
            println!("      display: {}", display.dimmed());
            println!("      utf16  : {}", units.join(" ").dimmed());
        }
    }

    /// One entry actually found on disk: its true leaf name (UTF-16, possibly
    /// ill-formed) and whether it is a directory.
    pub struct DiskEntry {
        /// True UTF-16 leaf name as stored by NTFS (may contain lone surrogates).
        pub name_utf16: Vec<u16>,
        /// `true` → directory.
        pub is_dir: bool,
    }

    impl DiskEntry {
        /// Lossy `&str` display form (U+FFFD for ill-formed units).
        pub fn display(&self) -> String {
            String::from_utf16_lossy(&self.name_utf16)
        }
        /// Space-separated hex of the raw UTF-16 code units.
        pub fn hex(&self) -> String {
            self.name_utf16
                .iter()
                .map(|u| format!("{u:04X}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
        /// `true` if the name has no valid UTF-8 form (an unpaired surrogate).
        pub fn is_ill_formed(&self) -> bool {
            has_unpaired_surrogate(&self.name_utf16)
        }
    }

    /// Enumerate the real entries in `root_win` via `FindFirstFileW`/
    /// `FindNextFileW`, skipping `.`/`..`. This reads the names the kernel
    /// actually stored, so ill-formed (surrogate-bearing) names survive verbatim
    /// — unlike Explorer, `dir`, or any `String`-based walk. Shared by `--list`
    /// and `--verify`.
    pub fn enumerate_on_disk(root: &str, root_win: &str) -> Result<Vec<DiskEntry>> {
        let pattern = format!(r"{root_win}\*");
        let pattern_z = wide_z(&pattern);

        // SAFETY: `find_data` is zeroed and the kernel fully initialises it on a
        // successful call; `pattern_z` is a valid NUL-terminated wide string.
        let mut find_data: ffi::Win32FindDataW = unsafe { core::mem::zeroed() };
        let handle = unsafe { ffi::FindFirstFileW(pattern_z.as_ptr(), &raw mut find_data) };

        if handle == ffi::INVALID_HANDLE_VALUE {
            // SAFETY: immediately after a failed Win32 call.
            let err = unsafe { ffi::GetLastError() };
            if err == ffi::ERROR_NO_MORE_FILES {
                return Ok(Vec::new());
            }
            anyhow::bail!(
                "FindFirstFileW on {root} failed (GetLastError={err}); does the folder exist? \
                 create it first (run without --list/--verify), or check the drive."
            );
        }

        let mut entries = Vec::new();
        loop {
            // The kernel writes a NUL-terminated name into a fixed 260-unit
            // buffer; take everything up to the first NUL.
            let leaf: Vec<u16> = find_data
                .cFileName
                .iter()
                .copied()
                .take_while(|&u| u != 0)
                .collect();
            // Skip the `.` and `..` pseudo-entries.
            let is_dot = matches!(leaf.as_slice(), [0x002E] | [0x002E, 0x002E]);
            if !is_dot {
                entries.push(DiskEntry {
                    is_dir: find_data.dwFileAttributes & ffi::FILE_ATTRIBUTE_DIRECTORY != 0,
                    name_utf16: leaf,
                });
            }
            // SAFETY: `handle` is valid; `find_data` is a live, owned struct.
            let more = unsafe { ffi::FindNextFileW(handle, &raw mut find_data) };
            if more == 0 {
                break;
            }
        }
        // SAFETY: `handle` came from a successful FindFirstFileW.
        unsafe { ffi::FindClose(handle) };
        Ok(entries)
    }

    /// Print each true on-disk name as display + raw UTF-16 hex, flagging
    /// ill-formed names. The "see the files without UFFS" path.
    pub fn list_on_disk(root: &str, root_win: &str) -> Result<()> {
        let entries = enumerate_on_disk(root, root_win)?;
        if entries.is_empty() {
            println!("{} {} is empty.", "list:".yellow().bold(), root.cyan());
            return Ok(());
        }

        println!(
            "{} true on-disk names in {} (read via FindFirstFileW — bypasses Explorer/dir mangling):",
            "list:".green().bold(),
            root.cyan()
        );
        for (i, e) in entries.iter().enumerate() {
            let kind = if e.is_dir { "dir " } else { "file" };
            let flag = if e.is_ill_formed() {
                " <ILL-FORMED UTF-16: unpaired surrogate>"
                    .red()
                    .bold()
                    .to_string()
            } else {
                String::new()
            };
            println!(
                "  {:>2}. [{}] {}{flag}",
                i + 1,
                kind.magenta(),
                e.display().dimmed()
            );
            println!("      utf16: {}", e.hex().dimmed());
        }

        let ill = entries.iter().filter(|e| e.is_ill_formed()).count();
        println!(
            "\n{} {} entries on disk ({ill} flagged {} — no UTF-8 form, the WI-4.4 target).",
            "total:".bold(),
            entries.len(),
            "ILL-FORMED".red().bold()
        );
        Ok(())
    }

    /// Does this UTF-16 sequence contain an unpaired surrogate (an ill-formed
    /// name with no valid UTF-8 representation)? A high surrogate
    /// (`0xD800..=0xDBFF`) is well-formed only when immediately followed by a
    /// low surrogate (`0xDC00..=0xDFFF`); any other surrogate is unpaired.
    fn has_unpaired_surrogate(units: &[u16]) -> bool {
        let mut i = 0_usize;
        while i < units.len() {
            let u = units[i];
            if (0xD800..=0xDBFF).contains(&u) {
                // High surrogate: needs a following low surrogate.
                match units.get(i + 1) {
                    Some(&low) if (0xDC00..=0xDFFF).contains(&low) => {
                        i += 2; // valid pair
                        continue;
                    }
                    _ => return true, // unpaired high
                }
            } else if (0xDC00..=0xDFFF).contains(&u) {
                return true; // lone low surrogate
            }
            i += 1;
        }
        false
    }

    /// Resolve the `uffs` binary: explicit `--bin`, then
    /// `%USERPROFILE%\bin\uffs.exe`, then `target\release\uffs.exe`, then bare
    /// `uffs.exe` on PATH (mirrors the sibling validation scripts so it dodges
    /// the `PATHEXT` `uffs.com` trap).
    fn resolve_bin(explicit: Option<&str>) -> String {
        if let Some(b) = explicit {
            return b.to_owned();
        }
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_owned());
        let candidates = [
            std::path::PathBuf::from(&home).join("bin").join("uffs.exe"),
            std::path::PathBuf::from("target").join("release").join("uffs.exe"),
        ];
        for c in &candidates {
            if c.exists() {
                return c.to_string_lossy().into_owned();
            }
        }
        "uffs.exe".to_owned()
    }

    /// Run `uffs search <marker>` and compare what UFFS finds against the actual
    /// on-disk entries. The on-disk enumeration (via `FindFirstFileW`) is the
    /// ground truth; UFFS's reported `name` is its lossy `&str` view, which is
    /// byte-for-byte the same lossy view this tool computes for each disk entry
    /// (`get_name() -> &str` in uffs-mft == `String::from_utf16_lossy` here), so
    /// the two are directly comparable. Any on-disk entry whose name UFFS does
    /// NOT report is a HIDDEN file — the WI-4.4 failure. Returns an error
    /// (non-zero exit) on any hidden entry.
    pub fn verify(
        root: &str,
        root_win: &str,
        drive: char,
        marker: &str,
        bin: Option<&str>,
    ) -> Result<()> {
        use std::process::Command;

        // 1. Ground truth: what is actually on disk?
        let disk = enumerate_on_disk(root, root_win)?;
        anyhow::ensure!(
            !disk.is_empty(),
            "{root} is empty — create the tree first (run without --verify)."
        );

        // 2. Ask UFFS. Generous limit so we never truncate the answer.
        let bin = resolve_bin(bin);
        let limit = (disk.len() * 4).max(200).to_string();
        let drive_s = drive.to_string();
        let args = [
            "search",
            marker,
            "--drives",
            &drive_s,
            "--filter",
            "all",
            "--limit",
            &limit,
            "--format",
            "json",
        ];
        println!(
            "{} {bin} {}",
            "verify:".bold(),
            args.join(" ").cyan()
        );
        let out = Command::new(&bin)
            .args(args)
            .output()
            .with_context(|| format!("running {bin} (is uffs on PATH, or pass --bin)?"))?;
        anyhow::ensure!(
            out.status.success(),
            "uffs search exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );

        // 3. Parse NDJSON (one JSON object per line) → set of reported names.
        // The `name` field is UFFS's lossy &str view; that is exactly what we
        // compare each disk entry's lossy display against.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut found_names: Vec<String> = Vec::new();
        let mut parse_errors = 0_usize;
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(v) => {
                    if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                        found_names.push(name.to_owned());
                    }
                }
                Err(_) => parse_errors += 1,
            }
        }
        if parse_errors > 0 {
            eprintln!(
                "  {} {parse_errors} UFFS output line(s) were not valid JSON (ignored).",
                "warn:".yellow()
            );
        }

        // 4. Cross-reference. For each on-disk entry, is its lossy display name
        // present in the UFFS results? (A marker-bearing file that UFFS does not
        // report is hidden.) We match on the leaf display; UFFS reports the leaf
        // name, so a contains-check on the reported name is exact for our leaves.
        let mut hidden: Vec<&DiskEntry> = Vec::new();
        for entry in &disk {
            let want = entry.display();
            let seen = found_names.iter().any(|got| got == &want);
            if !seen {
                hidden.push(entry);
            }
        }

        // 5. Report.
        println!();
        println!(
            "  on disk : {} entries ({} ill-formed)",
            disk.len().to_string().bold(),
            disk.iter().filter(|e| e.is_ill_formed()).count()
        );
        println!(
            "  UFFS    : {} entries reported for marker {}",
            found_names.len().to_string().bold(),
            marker.green()
        );
        println!();

        if !hidden.is_empty() {
            println!(
                "{} UFFS did NOT report {} of {} on-disk entries — these files are HIDDEN:",
                "FAIL:".red().bold(),
                hidden.len(),
                disk.len()
            );
            for e in &hidden {
                let kind = if e.is_dir { "dir " } else { "file" };
                let flag = if e.is_ill_formed() {
                    " <ILL-FORMED>".red().bold().to_string()
                } else {
                    String::new()
                };
                println!("    [{}] {}{flag}", kind.magenta(), e.display());
                println!("      utf16: {}", e.hex().dimmed());
            }
            anyhow::bail!(
                "{} on-disk entr{} hidden from UFFS — the WI-4.4 findability claim is VIOLATED",
                hidden.len(),
                if hidden.len() == 1 { "y is" } else { "ies are" }
            );
        }

        println!(
            "{} UFFS reported every on-disk entry — including all ill-formed names. \
             No file can hide behind a crooked name. {}",
            "PASS:".green().bold(),
            "(findability holds)".green()
        );

        // 6. Second proof — the `--malformed` filter. Every ILL-FORMED on-disk
        // entry must be returned by `uffs search * --malformed`, and the count
        // must match (the filter finds exactly the crooked names — no more, no
        // fewer). This exercises the WI-4.4-followup forensic filter directly.
        verify_malformed_filter(&disk, &bin, drive)
    }

    /// Cross-check the `uffs search * --malformed` filter against the
    /// ill-formed entries actually on disk.
    fn verify_malformed_filter(disk: &[DiskEntry], bin: &str, drive: char) -> Result<()> {
        use std::process::Command;

        let on_disk_ill: Vec<&DiskEntry> = disk.iter().filter(|e| e.is_ill_formed()).collect();
        let drive_s = drive.to_string();
        let args = [
            "search",
            "*",
            "--malformed",
            "--drives",
            &drive_s,
            "--filter",
            "all",
            "--limit",
            "1000",
            "--format",
            "json",
        ];
        println!();
        println!("{} {bin} {}", "verify:".bold(), args.join(" ").cyan());
        let out = Command::new(bin)
            .args(args)
            .output()
            .with_context(|| format!("running {bin} --malformed"))?;
        anyhow::ensure!(
            out.status.success(),
            "uffs search --malformed exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let returned = stdout.lines().filter(|l| !l.trim().is_empty()).count();

        println!(
            "  on disk : {} ill-formed entries",
            on_disk_ill.len().to_string().bold()
        );
        println!(
            "  --malformed returned: {} rows",
            returned.to_string().bold()
        );
        anyhow::ensure!(
            returned == on_disk_ill.len(),
            "--malformed returned {returned} rows but {} ill-formed entries are on disk — \
             the malformed filter is not matching exactly the crooked names",
            on_disk_ill.len()
        );
        println!(
            "{} `uffs search --malformed` returned exactly the {} crooked names. {}",
            "PASS:".green().bold(),
            on_disk_ill.len(),
            "(--malformed filter verified)".green()
        );
        Ok(())
    }

    /// Recursively delete the whole top-level folder. Uses `std::fs` on the
    /// `\\?\` path; Rust's `remove_dir_all` walks via the wide API and so can
    /// delete the surrogate-named children it created.
    pub fn cleanup(root: &str, root_win: &str) -> Result<()> {
        match std::fs::remove_dir_all(root_win) {
            Ok(()) => {
                println!("{} removed {}", "cleanup:".green().bold(), root.cyan());
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!(
                    "{} nothing to remove ({} does not exist)",
                    "cleanup:".yellow().bold(),
                    root.cyan()
                );
                Ok(())
            }
            Err(e) => Err(e).with_context(|| format!("removing {root}")),
        }
    }
}
