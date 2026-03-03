//! Process list from /proc/[pid]/stat and /proc/[pid]/status.
//!
//! We iterate /proc with opendir/readdir (via std::fs::read_dir) and read each
//! process's stat file into a per-iteration 512-byte stack buffer.
//!
//! CPU% calculation:
//!   delta_jiffies = (utime + stime) - (prev_utime + prev_stime)
//!   elapsed_jiffies = Δtime_secs * CLK_TCK   (usually 100 Hz)
//!   cpu% = delta_jiffies / elapsed_jiffies * 100
//!
//! On macOS (with --features macos), process listing is a stub since libproc
//! APIs aren't available in the standard libc crate.

#[cfg(not(all(feature = "macos", target_os = "macos")))]
use crate::cpu::{parse_u64, read_file};
use crate::types::ProcessInfo;

#[cfg(not(all(feature = "macos", target_os = "macos")))]
use std::collections::HashMap;

/// CLK_TCK — almost always 100 on Linux. We read it once via sysconf.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
static mut CLK_TCK: u64 = 100;

#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn init() {
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 {
        unsafe { CLK_TCK = hz as u64 };
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn init() {
    // No initialization needed on macOS
}

/// Update the process list.
///
/// * `procs`      – mutable list that persists between calls (stores prev jiffies)
/// * `mem_total_kb` – for mem% calculation
/// * `elapsed_secs` – seconds since last call (for CPU% normalisation)
/// * `top_n`      – how many processes to return (sorted by CPU descending)
/// * `filter_pid` – if Some, only include that PID
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn update(
    procs: &mut Vec<ProcessInfo>,
    mem_total_kb: u64,
    elapsed_secs: f32,
    top_n: usize,
    filter_pid: Option<u32>,
) {
    let clk = unsafe { CLK_TCK } as f32;
    let elapsed_jiffies = (elapsed_secs * clk).max(1.0);

    // Build a lookup of pid → old jiffies from the previous snapshot.
    let mut prev_map: HashMap<u32, (u64, u64)> = HashMap::with_capacity(procs.len());
    for p in procs.iter() {
        prev_map.insert(p.pid, (p.prev_utime, p.prev_stime));
    }
    procs.clear();

    // Walk /proc looking for numeric directory names (PIDs).
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };

    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname_bytes = fname.as_encoded_bytes();
        // Skip non-numeric entries
        if fname_bytes.is_empty() || !fname_bytes[0].is_ascii_digit() {
            continue;
        }
        let Some(pid) = parse_u64(fname_bytes).map(|v| v as u32) else {
            continue;
        };
        if let Some(wanted) = filter_pid {
            if pid != wanted { continue; }
        }

        let mut info = ProcessInfo::default();
        info.pid = pid;

        if !read_proc_stat(pid, &mut info) {
            continue; // process may have exited
        }
        read_proc_mem(pid, &mut info);

        // CPU%
        let (prev_u, prev_s) = prev_map.get(&pid).copied().unwrap_or((0, 0));
        let cur_jiffies = info.prev_utime + info.prev_stime;
        let old_jiffies = prev_u + prev_s;
        let delta = cur_jiffies.saturating_sub(old_jiffies) as f32;
        info.cpu_usage = (delta / elapsed_jiffies * 100.0).min(100.0 * 256.0);

        // MEM%
        if mem_total_kb > 0 {
            info.mem_percent = info.mem_kb as f32 / mem_total_kb as f32 * 100.0;
        }

        procs.push(info);
    }

    // Sort by CPU descending; take top_n.
    procs.sort_unstable_by(|a, b| b.cpu_usage.partial_cmp(&a.cpu_usage).unwrap());
    procs.truncate(top_n);
}

/// macOS implementation - simplified stub as libproc isn't in standard libc.
/// On macOS, process listing requires private Apple APIs not available in libc crate.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn update(
    procs: &mut Vec<ProcessInfo>,
    _mem_total_kb: u64,
    _elapsed_secs: f32,
    _top_n: usize,
    _filter_pid: Option<u32>,
) {
    // Process listing on macOS requires:
    // - proc_listpids/proc_pidinfo from libproc (not in libc crate)
    // - Or sysctl with CTL_KERN/KERN_PROC/KERN_PROC_ALL
    // 
    // For development purposes, returning empty list.
    // Full implementation would need bindings to libproc.dylib
    procs.clear();
}

/// Returns a slice sorted by memory descending (re-sorts in place).
#[allow(dead_code)]
pub fn sort_by_mem(procs: &mut [ProcessInfo]) {
    procs.sort_unstable_by(|a, b| b.mem_kb.cmp(&a.mem_kb));
}

// ─── /proc/[pid]/stat ────────────────────────────────────────────────────────

/// Parse /proc/[pid]/stat into `info`. Returns false if the file is unreadable.
///
/// Format: pid (comm) state ppid pgrp sess tty_nr tpgid flags minflt cminflt
///         majflt cmajflt utime stime cutime cstime prio nice …
/// Fields are 1-indexed in the kernel docs; here we count from 0.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_proc_stat(pid: u32, info: &mut ProcessInfo) -> bool {
    let path = format!("/proc/{}/stat", pid);
    let mut buf = [0u8; 512];
    let n = read_file(&path, &mut buf);
    if n == 0 { return false; }
    let data = &buf[..n];

    // Name is between '(' and ')' and can contain spaces.
    let name_start = data.iter().position(|&b| b == b'(');
    let name_end = data.iter().rposition(|&b| b == b')');
    let (name_start, name_end) = match (name_start, name_end) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return false,
    };

    let name = &data[name_start + 1..name_end];
    let copy_len = name.len().min(info.name.len());
    info.name[..copy_len].copy_from_slice(&name[..copy_len]);
    info.name_len = copy_len;

    // Everything after ')' is space-separated fields starting at index 2 (state).
    let rest = &data[name_end + 1..];
    let mut fields = rest.split(|&b| b == b' ').filter(|s| !s.is_empty());

    let state = fields.next().and_then(|s| s.first().copied()).unwrap_or(b'?');
    info.status = state;

    // Skip: ppid pgrp sess tty_nr tpgid flags minflt cminflt majflt cmajflt
    // That's 10 fields (indices 3–12).
    for _ in 0..10 { fields.next(); }

    // utime (index 13), stime (14)
    let utime = fields.next().and_then(|s| parse_u64(s)).unwrap_or(0);
    let stime = fields.next().and_then(|s| parse_u64(s)).unwrap_or(0);

    info.prev_utime = utime;
    info.prev_stime = stime;

    true
}

// ─── /proc/[pid]/status ──────────────────────────────────────────────────────

/// Read VmRSS from /proc/[pid]/status for memory usage in KiB.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_proc_mem(pid: u32, info: &mut ProcessInfo) {
    let path = format!("/proc/{}/status", pid);
    let mut buf = [0u8; 2048];
    let n = read_file(&path, &mut buf);
    let data = &buf[..n];

    for line in data.split(|&b| b == b'\n') {
        if line.starts_with(b"VmRSS:") {
            let rest = &line[6..];
            info.mem_kb = parse_first_u64_in(rest);
            return;
        }
    }
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn parse_first_u64_in(b: &[u8]) -> u64 {
    let mut n = 0u64;
    let mut found = false;
    for &c in b {
        if c.is_ascii_digit() {
            n = n.wrapping_mul(10).wrapping_add((c - b'0') as u64);
            found = true;
        } else if found {
            break;
        }
    }
    n
}

#[cfg(test)]
#[cfg(not(all(feature = "macos", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_proc_stat_format() {
        // Simulate parsing the name extraction logic
        let data = b"1234 (my process) S 1 1234 1234 0 -1 4194304 100 0 0 0 50 20 0 0 20 0 1";
        let name_start = data.iter().position(|&b| b == b'(').unwrap();
        let name_end = data.iter().rposition(|&b| b == b')').unwrap();
        let name = &data[name_start + 1..name_end];
        assert_eq!(name, b"my process");
    }

    #[test]
    fn test_parse_first_u64_in() {
        assert_eq!(parse_first_u64_in(b"   12345 kB"), 12345);
        assert_eq!(parse_first_u64_in(b"0"), 0);
    }
}
