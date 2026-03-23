#!/usr/bin/env rust-script
//! UFFS Live Parity Check — C++ vs Rust comparison.
//!
//! Usage:
//!   rust-script scripts/windows/parity_check.rs D [E F] [--bin-dir DIR] [--sample N]
//!   rust-script scripts/windows/parity_check.rs C --pattern "*.txt"
//!   rust-script scripts/windows/parity_check.rs C --pattern ">C:\\Users\\.*\.(jpg|png)"
//! ```cargo
//! [dependencies]
//! sha2 = "0.10"
//! ```

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

struct Config { drives: Vec<String>, pattern: String, cpp: PathBuf, rust: PathBuf, sample: usize, out: PathBuf }
struct Scan { ok: bool, ms: u128, err: Option<String> }

fn main() {
    let cfg = parse_args();
    println!("\n╔════════════════════════════════════════════╗");
    println!("║   UFFS Live Parity Check — C++ vs Rust     ║");
    println!("╚════════════════════════════════════════════╝\n");
    println!("  C++ : {}\n  Rust: {}\n  Drives: {}\n  Pattern: {}\n", cfg.cpp.display(), cfg.rust.display(), cfg.drives.join(", "), cfg.pattern);
    let ok = cfg.drives.iter().all(|d| check(&cfg, d));
    std::process::exit(if ok { 0 } else { 1 });
}

fn parse_args() -> Config {
    let args: Vec<String> = env::args().collect();
    let mut drives = vec![]; let mut bin = home().join("bin"); let mut sample = 30usize;
    let mut pattern = "*".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bin-dir" => { i += 1; bin = PathBuf::from(&args[i]); }
            "--sample" => { i += 1; sample = args[i].parse().unwrap_or(30); }
            "--pattern" => { i += 1; pattern = args[i].clone(); }
            a if !a.starts_with('-') => drives.push(a.to_uppercase()),
            _ => {}
        }
        i += 1;
    }
    if drives.is_empty() { eprintln!("Usage: parity_check.rs <DRIVE> [--pattern PAT] [--bin-dir DIR] [--sample N]"); std::process::exit(1); }
    Config { drives, pattern, cpp: bin.join("uffs.com"), rust: bin.join("uffs.exe"), sample, out: env::current_dir().unwrap() }
}

fn home() -> PathBuf { env::var_os("USERPROFILE").or(env::var_os("HOME")).map(PathBuf::from).unwrap_or(".".into()) }
fn ts() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() }
fn flush() { std::io::stdout().flush().ok(); }

fn check(cfg: &Config, drv: &str) -> bool {
    let dl = drv.to_lowercase(); let t = ts();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n  Drive {}\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", drv);
    let cpp_f = cfg.out.join(format!("parity_cpp_{dl}_{t}.txt"));
    let rust_f = cfg.out.join(format!("parity_rust_{dl}_{t}.txt"));

    let cpp_drives_arg = format!("--drives={}", drv);
    print!("  [1/4] C++ scan..."); flush();
    let c = scan(&cfg.cpp, &[&cfg.pattern, &cpp_drives_arg], &cpp_f); pres(&c);
    print!("  [2/4] Rust scan..."); flush();
    let r = scan(&cfg.rust, &[&cfg.pattern, "--drive", drv, "--no-cache", "--format", "custom"], &rust_f); pres(&r);

    if !c.ok || !r.ok { println!("  ❌ Scan failed"); return false; }

    print!("  [3/4] Sorting..."); flush();
    let t0 = Instant::now();
    let cl = rsort(&cpp_f); let rl = rsort(&rust_f);
    println!(" ✅ ({} ms)\n    C++: {} lines, Rust: {} lines", t0.elapsed().as_millis(), cl.len(), rl.len());

    print!("  [4/4] SHA256..."); flush();
    let ch = sha(&cl); let rh = sha(&rl);
    if ch == rh {
        println!(" ✅ MATCH\n\n  ╔══════════════════════════════════════════╗\n  ║  PARITY: PASS                            ║\n  ╚══════════════════════════════════════════╝\n    SHA256: {}\n", ch);
        fs::remove_file(&cpp_f).ok(); fs::remove_file(&rust_f).ok(); true
    } else {
        println!(" ❌ MISMATCH"); diff(cfg, drv, &cl, &rl, &ch, &rh, t); false
    }
}

fn scan(bin: &PathBuf, args: &[&str], out: &PathBuf) -> Scan {
    let t0 = Instant::now();
    match Command::new(bin).args(args).output() {
        Ok(o) if o.status.success() => { fs::write(out, &o.stdout).ok(); Scan { ok: true, ms: t0.elapsed().as_millis(), err: None } }
        Ok(o) => Scan { ok: false, ms: t0.elapsed().as_millis(), err: Some(format!("exit {}: {}", o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stderr))) },
        Err(e) => Scan { ok: false, ms: t0.elapsed().as_millis(), err: Some(e.to_string()) },
    }
}

fn pres(s: &Scan) {
    if s.ok { println!(" ✅ ({} ms)", s.ms); }
    else { println!(" ❌ ({} ms)", s.ms); if let Some(e) = &s.err { for l in e.lines().take(3) { println!("    {}", l); } } }
}

fn rsort(p: &PathBuf) -> Vec<String> {
    let mut v: Vec<String> = BufReader::new(File::open(p).unwrap()).lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).collect();
    v.sort_unstable(); v
}

fn sha(lines: &[String]) -> String {
    let mut h = Sha256::new(); for l in lines { h.update(l.as_bytes()); h.update(b"\n"); } format!("{:x}", h.finalize())
}

fn diff(cfg: &Config, drv: &str, cpp: &[String], rust: &[String], ch: &str, rh: &str, t: u64) {
    println!("\n  ╔══════════════════════════════════════════╗\n  ║  PARITY: FAIL                            ║\n  ╚══════════════════════════════════════════╝");
    println!("    C++ SHA256 : {}\n    Rust SHA256: {}", ch, rh);

    // Find and show header (first line, usually column names)
    let hdr = cpp.first().map(|s| s.as_str()).unwrap_or("");
    if !hdr.is_empty() && hdr.contains("Path") {
        println!("\n    Header: {}", hdr);
    }

    let cs: HashSet<&str> = cpp.iter().map(|s| s.as_str()).collect();
    let rs: HashSet<&str> = rust.iter().map(|s| s.as_str()).collect();
    let oc: Vec<_> = cpp.iter().filter(|l| !rs.contains(l.as_str())).collect();
    let or: Vec<_> = rust.iter().filter(|l| !cs.contains(l.as_str())).collect();
    println!("\n    Only C++: {} | Only Rust: {}", oc.len(), or.len());

    // Show full lines, no truncation
    if !oc.is_empty() {
        println!("\n    C++ only (first 5):");
        for l in oc.iter().take(5) { println!("      < {}", l); }
    }
    if !or.is_empty() {
        println!("\n    Rust only (first 5):");
        for l in or.iter().take(5) { println!("      > {}", l); }
    }

    let dp = cfg.out.join(format!("parity_diff_{}_{}.txt", drv.to_lowercase(), t));
    if let Ok(mut f) = File::create(&dp) {
        writeln!(f, "# Drive {} | C++ SHA256: {} | Rust SHA256: {}", drv, ch, rh).ok();
        writeln!(f, "# C++ lines: {} | Rust lines: {} | Only C++: {} | Only Rust: {}", cpp.len(), rust.len(), oc.len(), or.len()).ok();
        if !hdr.is_empty() { writeln!(f, "# Header: {}\n", hdr).ok(); }
        writeln!(f, "=== C++ ONLY ({}) ===", oc.len()).ok();
        for l in oc.iter().take(cfg.sample) { writeln!(f, "< {}", l).ok(); }
        writeln!(f, "\n=== RUST ONLY ({}) ===", or.len()).ok();
        for l in or.iter().take(cfg.sample) { writeln!(f, "> {}", l).ok(); }
        println!("\n    Diff: {}", dp.display());
    }
    println!();
}

