//! Memory statistics from /proc/meminfo.
//!
//! All parsing is done on a 2 KB stack buffer — /proc/meminfo is always
//! well under 2 KB in practice even on systems with huge NUMA topologies.
//!
//! On macOS (with --features macos), uses host_statistics64.

#[cfg(not(all(feature = "macos", target_os = "macos")))]
use crate::cpu::read_file;
use crate::types::MemStats;

/// Populate `stats` from a fresh /proc/meminfo read.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn update(stats: &mut MemStats) {
    let mut buf = [0u8; 2048];
    let n = read_file("/proc/meminfo", &mut buf);
    parse_meminfo(&buf[..n], stats);
}

/// macOS implementation using host_statistics64.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn update(stats: &mut MemStats) {
    use std::mem::MaybeUninit;

    // Get page size
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;

    // Get total memory via sysctl
    let mut total_bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = b"hw.memsize\0";
    unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const i8,
            &mut total_bytes as *mut u64 as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        );
    }
    stats.total_kb = total_bytes / 1024;

    // Get VM statistics
    #[allow(deprecated)]
    let host = unsafe { libc::mach_host_self() };
    let mut vm_stat: libc::vm_statistics64_data_t = unsafe { MaybeUninit::zeroed().assume_init() };
    let mut count =
        (std::mem::size_of::<libc::vm_statistics64_data_t>() / std::mem::size_of::<i32>()) as u32;

    let ret = unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64 as i32,
            &mut vm_stat as *mut _ as *mut i32,
            &mut count,
        )
    };

    if ret == libc::KERN_SUCCESS as i32 {
        let free_pages = vm_stat.free_count as u64;
        let active_pages = vm_stat.active_count as u64;
        let inactive_pages = vm_stat.inactive_count as u64;
        let wired_pages = vm_stat.wire_count as u64;
        let speculative_pages = vm_stat.speculative_count as u64;
        let purgeable_pages = vm_stat.purgeable_count as u64;

        stats.free_kb = (free_pages * page_size) / 1024;
        stats.cached_kb =
            ((inactive_pages + purgeable_pages + speculative_pages) * page_size) / 1024;
        stats.buffers_kb = 0; // macOS doesn't separate buffers

        // Available = free + inactive + purgeable
        stats.available_kb = ((free_pages + inactive_pages + purgeable_pages) * page_size) / 1024;

        // Used = active + wired
        stats.used_kb = ((active_pages + wired_pages) * page_size) / 1024;
    }

    // Get swap info via sysctl
    let mut xsu: libc::xsw_usage = unsafe { MaybeUninit::zeroed().assume_init() };
    let mut xsu_len = std::mem::size_of::<libc::xsw_usage>();
    let swap_name = b"vm.swapusage\0";
    let swap_ret = unsafe {
        libc::sysctlbyname(
            swap_name.as_ptr() as *const i8,
            &mut xsu as *mut _ as *mut libc::c_void,
            &mut xsu_len,
            std::ptr::null_mut(),
            0,
        )
    };

    if swap_ret == 0 {
        stats.swap_total_kb = xsu.xsu_total / 1024;
        stats.swap_used_kb = xsu.xsu_used / 1024;
        stats.swap_free_kb = xsu.xsu_avail / 1024;
    }
}

/// Parse /proc/meminfo key: value kB lines into `stats`.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn parse_meminfo(data: &[u8], stats: &mut MemStats) {
    for line in data.split(|&b| b == b'\n') {
        // Format: "KeyName:   12345 kB"
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let key = &line[..colon];
        let rest = &line[colon + 1..];
        // Parse the first integer in rest
        let value = parse_first_u64(rest);

        match key {
            b"MemTotal" => stats.total_kb = value,
            b"MemFree" => stats.free_kb = value,
            b"MemAvailable" => stats.available_kb = value,
            b"Buffers" => stats.buffers_kb = value,
            b"Cached" => {
                // "Cached" line appears before "SwapCached" — guard against that.
                if stats.cached_kb == 0 {
                    stats.cached_kb = value;
                }
            }
            b"SwapTotal" => stats.swap_total_kb = value,
            b"SwapFree" => stats.swap_free_kb = value,
            _ => {}
        }
    }

    // Compute derived fields.
    stats.used_kb = stats
        .total_kb
        .saturating_sub(stats.free_kb)
        .saturating_sub(stats.buffers_kb)
        .saturating_sub(stats.cached_kb);
    stats.swap_used_kb = stats.swap_total_kb.saturating_sub(stats.swap_free_kb);
}

/// Scan bytes for the first ASCII decimal number and return it.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn parse_first_u64(b: &[u8]) -> u64 {
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

    const MOCK: &[u8] = b"MemTotal:       32768000 kB\n\
                           MemFree:        16384000 kB\n\
                           MemAvailable:   20000000 kB\n\
                           Buffers:          512000 kB\n\
                           Cached:          3000000 kB\n\
                           SwapTotal:       8000000 kB\n\
                           SwapFree:        7000000 kB\n";

    #[test]
    fn test_parse_meminfo() {
        let mut s = MemStats::default();
        parse_meminfo(MOCK, &mut s);
        assert_eq!(s.total_kb, 32768000);
        assert_eq!(s.free_kb, 16384000);
        assert_eq!(s.available_kb, 20000000);
        assert_eq!(s.buffers_kb, 512000);
        assert_eq!(s.cached_kb, 3000000);
        assert_eq!(s.swap_total_kb, 8000000);
        assert_eq!(s.swap_free_kb, 7000000);
        assert_eq!(s.swap_used_kb, 1000000);
        // used = total - free - buffers - cached
        assert_eq!(s.used_kb, 32768000 - 16384000 - 512000 - 3000000);
    }
}
