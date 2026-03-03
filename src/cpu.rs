//! CPU statistics from /proc/stat, /sys/devices/system/cpu, and /sys/class/thermal.
//!
//! Design: All reads use a fixed-size stack buffer.  The jiffie-delta between
//! two successive calls to `update()` yields the per-core CPU percentage.
//!
//! Jiffie accounting (from kernel docs):
//!   total  = user + nice + system + idle + iowait + irq + softirq + steal
//!   active = total - idle - iowait   (mirrors htop definition)
//!   cpu%   = (Δactive / Δtotal) * 100
//!
//! On macOS (with --features macos), uses sysctl and host_processor_info.

use crate::types::{CpuCore, CpuStats};

#[cfg(not(all(feature = "macos", target_os = "macos")))]
use std::fs::File;
#[cfg(not(all(feature = "macos", target_os = "macos")))]
use std::io::Read;

/// Fill `stats` with fresh data.  Call this once to initialise, then again
/// after at least one tick to get meaningful percentages.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn update(stats: &mut CpuStats) {
    read_cpuinfo(stats);
    read_stat(stats);
    read_frequencies(stats);
    read_temperatures(stats);
}

// ─── /proc/cpuinfo ──────────────────────────────────────────────────────────

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_cpuinfo(stats: &mut CpuStats) {
    if stats.model_len > 0 {
        // Model name doesn't change; only read it once.
        return;
    }
    let mut buf = [0u8; 4096];
    let n = read_file("/proc/cpuinfo", &mut buf);
    let content = &buf[..n];

    // Find "model name\t: <value>\n"
    if let Some(pos) = find_subsequence(content, b"model name") {
        let rest = &content[pos..];
        if let Some(colon) = rest.iter().position(|&b| b == b':') {
            let after = &rest[colon + 1..];
            let end = after
                .iter()
                .position(|&b| b == b'\n')
                .unwrap_or(after.len());
            let name = trim_bytes(&after[..end]);
            let copy_len = name.len().min(stats.model.len() - 1);
            stats.model[..copy_len].copy_from_slice(&name[..copy_len]);
            stats.model_len = copy_len;
        }
    }
}

// ─── /proc/stat ─────────────────────────────────────────────────────────────

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_stat(stats: &mut CpuStats) {
    // /proc/stat can be large on many-core systems; 16 KB covers 256 cores.
    let mut buf = [0u8; 16384];
    let n = read_file("/proc/stat", &mut buf);
    let content = &buf[..n];

    let mut lines = content.split(|&b| b == b'\n');

    // First line: aggregate "cpu  ..."
    if let Some(line) = lines.next() {
        if line.starts_with(b"cpu ") || line.starts_with(b"cpu\t") {
            let (total, idle) = parse_stat_line(&line[3..]);
            let delta_total = total.saturating_sub(stats.prev_total);
            let delta_idle = idle.saturating_sub(stats.prev_idle);
            stats.total_usage = if delta_total > 0 {
                (1.0 - delta_idle as f32 / delta_total as f32) * 100.0
            } else {
                0.0
            };
            stats.prev_total = total;
            stats.prev_idle = idle;
        }
    }

    // Subsequent lines: "cpu0 ...", "cpu1 ..." etc.
    let mut core_idx = 0usize;
    for line in lines {
        if !line.starts_with(b"cpu") || line.len() < 4 {
            break; // past cpu lines
        }
        // Skip "cpu " aggregate already handled above
        let digit_start = 3;
        if !line[digit_start].is_ascii_digit() {
            continue;
        }

        let (total, idle) = parse_stat_line(skip_word(line));

        if core_idx >= stats.cores.len() {
            stats.cores.push(CpuCore {
                id: core_idx as u32,
                ..Default::default()
            });
        }
        let core = &mut stats.cores[core_idx];
        let delta_total = total.saturating_sub(core.prev_total);
        let delta_idle = idle.saturating_sub(core.prev_idle);
        core.usage = if delta_total > 0 {
            (1.0 - delta_idle as f32 / delta_total as f32) * 100.0
        } else {
            0.0
        };
        core.prev_total = total;
        core.prev_idle = idle;
        core_idx += 1;
    }
}

/// Parse space-separated jiffie fields from a cpu line (after the cpu label).
/// Returns (total_jiffies, idle_jiffies).
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn parse_stat_line(line: &[u8]) -> (u64, u64) {
    let mut fields = [0u64; 10];
    let mut idx = 0;
    let mut num = 0u64;
    let mut in_num = false;

    for &b in line {
        if b.is_ascii_digit() {
            num = num.wrapping_mul(10).wrapping_add((b - b'0') as u64);
            in_num = true;
        } else if in_num {
            if idx < 10 {
                fields[idx] = num;
            }
            idx += 1;
            num = 0;
            in_num = false;
            if idx == 10 {
                break;
            }
        }
    }
    if in_num && idx < 10 {
        fields[idx] = num;
    }

    // user nice system idle iowait irq softirq steal guest guest_nice
    let idle = fields[3] + fields[4]; // idle + iowait
    let total: u64 = fields[..8].iter().sum();
    (total, idle)
}

// ─── CPU frequencies ─────────────────────────────────────────────────────────

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_frequencies(stats: &mut CpuStats) {
    let mut buf = [0u8; 32];
    for core in &mut stats.cores {
        // Path: /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq
        let path_str = format!(
            "/sys/devices/system/cpu/cpu{}/cpufreq/scaling_cur_freq",
            core.id
        );
        let n = read_file(&path_str, &mut buf);
        if n > 0 {
            if let Some(khz) = parse_u64(&buf[..n]) {
                core.freq_mhz = (khz / 1000) as u32;
            }
        }
    }
}

// ─── CPU temperatures ────────────────────────────────────────────────────────

/// Read thermal_zone temperatures and assign them to cores heuristically.
/// Only the first available zone is used for a single temperature reading;
/// a more sophisticated mapping is skipped to avoid complexity bloat.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_temperatures(stats: &mut CpuStats) {
    let mut buf = [0u8; 16];

    // Try thermal_zone0 first; iterate up to 16 zones.
    for zone in 0..16u32 {
        let path = format!("/sys/class/thermal/thermal_zone{}/temp", zone);
        let n = read_file(&path, &mut buf);
        if n == 0 {
            continue;
        }

        // Check type to see if it's a CPU zone
        let type_path = format!("/sys/class/thermal/thermal_zone{}/type", zone);
        let mut type_buf = [0u8; 64];
        let tn = read_file(&type_path, &mut type_buf);
        let zone_type = trim_bytes(&type_buf[..tn]);
        let is_cpu = zone_type.starts_with(b"x86_pkg")
            || zone_type.starts_with(b"acpitz")
            || zone_type.starts_with(b"cpu")
            || zone_type.starts_with(b"CPU");
        if !is_cpu && zone > 0 {
            continue;
        }

        if let Some(millic) = parse_u64(&buf[..n]) {
            let celsius = millic as f32 / 1000.0;
            // Assign this temperature to all cores (package temp approximation).
            for core in &mut stats.cores {
                core.temp_c = Some(celsius);
            }
            return; // one temperature reading is enough
        }
    }
}

// ─── utilities ───────────────────────────────────────────────────────────────

/// Read a file into `buf`. Returns bytes read, or 0 on error.
/// Uses a stack-local File that is dropped immediately — no heap allocation
/// beyond the OS file-descriptor table entry.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn read_file(path: &str, buf: &mut [u8]) -> usize {
    let Ok(mut f) = File::open(path) else {
        return 0;
    };
    f.read(buf).unwrap_or(0)
}

/// Stub read_file for macOS — returns 0 as /proc doesn't exist.
#[cfg(all(feature = "macos", target_os = "macos"))]
#[allow(dead_code)]
pub fn read_file(_path: &str, _buf: &mut [u8]) -> usize {
    0
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn skip_word(b: &[u8]) -> &[u8] {
    let pos = b
        .iter()
        .position(|c| c.is_ascii_whitespace())
        .unwrap_or(b.len());
    &b[pos..]
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[allow(dead_code)]
fn trim_bytes(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|&c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|&c| !c.is_ascii_whitespace())
        .map(|p| p + 1)
        .unwrap_or(0);
    if start >= end {
        b""
    } else {
        &b[start..end]
    }
}

#[allow(dead_code)]
pub fn parse_u64(b: &[u8]) -> Option<u64> {
    let b = trim_bytes(b);
    if b.is_empty() {
        return None;
    }
    let mut n = 0u64;
    for &c in b {
        if !c.is_ascii_digit() {
            break;
        }
        n = n.wrapping_mul(10).wrapping_add((c - b'0') as u64);
    }
    Some(n)
}

// ─── macOS implementation ─────────────────────────────────────────────────────

#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn update(stats: &mut CpuStats) {
    macos_read_cpu_info(stats);
    macos_read_cpu_usage(stats);
}

/// Read CPU model via sysctl on macOS.
#[cfg(all(feature = "macos", target_os = "macos"))]
fn macos_read_cpu_info(stats: &mut CpuStats) {
    if stats.model_len > 0 {
        return; // Already read
    }

    let mut buf = [0u8; 128];
    let mut len = buf.len();

    let name = b"machdep.cpu.brand_string\0";
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const i8,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };

    if ret == 0 && len > 0 {
        let copy_len = (len - 1).min(stats.model.len() - 1);
        stats.model[..copy_len].copy_from_slice(&buf[..copy_len]);
        stats.model_len = copy_len;
    }
}

/// Read CPU usage via host_processor_info on macOS.
#[cfg(all(feature = "macos", target_os = "macos"))]
fn macos_read_cpu_usage(stats: &mut CpuStats) {
    use std::mem::MaybeUninit;

    // Get number of CPUs
    let mut ncpu: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let name = b"hw.ncpu\0";
    unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const i8,
            &mut ncpu as *mut i32 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        );
    }

    if ncpu <= 0 {
        ncpu = 1;
    }

    // Ensure we have enough cores
    while stats.cores.len() < ncpu as usize {
        stats.cores.push(CpuCore {
            id: stats.cores.len() as u32,
            ..Default::default()
        });
    }

    // Get CPU load info via host_statistics
    #[allow(deprecated)]
    let host = unsafe { libc::mach_host_self() };

    let mut cpu_load: libc::host_cpu_load_info_data_t =
        unsafe { MaybeUninit::zeroed().assume_init() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT as u32;

    let ret = unsafe {
        libc::host_statistics(
            host,
            libc::HOST_CPU_LOAD_INFO as i32,
            &mut cpu_load as *mut _ as *mut i32,
            &mut count,
        )
    };

    if ret == libc::KERN_SUCCESS as i32 {
        let user = cpu_load.cpu_ticks[libc::CPU_STATE_USER as usize] as u64;
        let system = cpu_load.cpu_ticks[libc::CPU_STATE_SYSTEM as usize] as u64;
        let idle = cpu_load.cpu_ticks[libc::CPU_STATE_IDLE as usize] as u64;
        let nice = cpu_load.cpu_ticks[libc::CPU_STATE_NICE as usize] as u64;

        let total = user + system + idle + nice;
        let _active = user + system + nice;

        let delta_total = total.saturating_sub(stats.prev_total);
        let delta_idle = idle.saturating_sub(stats.prev_idle);

        stats.total_usage = if delta_total > 0 {
            (1.0 - delta_idle as f32 / delta_total as f32) * 100.0
        } else {
            0.0
        };

        stats.prev_total = total;
        stats.prev_idle = idle;

        // Apply aggregate usage to all cores (simplified)
        let per_core = stats.total_usage;
        for core in &mut stats.cores {
            core.usage = per_core;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    #[test]
    fn test_parse_stat_line() {
        // user=100 nice=0 system=20 idle=880 iowait=0 irq=0 softirq=0 steal=0
        let line = b" 100 0 20 880 0 0 0 0 0 0";
        let (total, idle) = parse_stat_line(line);
        assert_eq!(total, 1000);
        assert_eq!(idle, 880);
    }

    #[test]
    fn test_parse_u64() {
        assert_eq!(parse_u64(b"12345"), Some(12345));
        assert_eq!(parse_u64(b"  42\n"), Some(42));
        assert_eq!(parse_u64(b""), None);
    }
}
