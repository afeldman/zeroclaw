//! System-wide metadata: uptime, load average, hostname, kernel version.
//!
//! All data is read from /proc on each call; these are cheap reads that
//! the kernel serves directly from memory without disk I/O.
//!
//! On macOS (with --features macos), uses sysctl.

#[cfg(not(all(feature = "macos", target_os = "macos")))]
use crate::cpu::read_file;
use crate::types::SysInfo;

/// Populate `info` with fresh system metadata.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn update(info: &mut SysInfo) {
    read_uptime(info);
    read_loadavg(info);
    read_hostname(info);
    read_kernel(info);
}

/// macOS implementation using sysctl.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn update(info: &mut SysInfo) {
    macos_read_uptime(info);
    macos_read_loadavg(info);
    macos_read_hostname(info);
    macos_read_kernel(info);
}

#[cfg(all(feature = "macos", target_os = "macos"))]
fn macos_read_uptime(info: &mut SysInfo) {
    use std::mem::MaybeUninit;

    let mut boottime: libc::timeval = unsafe { MaybeUninit::zeroed().assume_init() };
    let mut len = std::mem::size_of::<libc::timeval>();
    let name = b"kern.boottime\0";

    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const i8,
            &mut boottime as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };

    if ret == 0 {
        let mut now: libc::timeval = unsafe { MaybeUninit::zeroed().assume_init() };
        unsafe { libc::gettimeofday(&mut now, std::ptr::null_mut()) };
        info.uptime_secs = (now.tv_sec - boottime.tv_sec) as u64;
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
fn macos_read_loadavg(info: &mut SysInfo) {
    let mut loadavg = [0f64; 3];
    let ret = unsafe { libc::getloadavg(loadavg.as_mut_ptr(), 3) };
    if ret == 3 {
        info.load_1 = loadavg[0] as f32;
        info.load_5 = loadavg[1] as f32;
        info.load_15 = loadavg[2] as f32;
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
fn macos_read_hostname(info: &mut SysInfo) {
    let mut buf = [0u8; 64];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut i8, buf.len()) };
    if ret == 0 {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let copy_len = len.min(info.hostname.len());
        info.hostname[..copy_len].copy_from_slice(&buf[..copy_len]);
        info.hostname_len = copy_len;
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
fn macos_read_kernel(info: &mut SysInfo) {
    if info.kernel_len > 0 {
        return;
    }

    let mut buf = [0u8; 128];
    let mut len = buf.len();
    let name = b"kern.osrelease\0";

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
        let copy_len = (len - 1).min(info.kernel.len());
        info.kernel[..copy_len].copy_from_slice(&buf[..copy_len]);
        info.kernel_len = copy_len;
    }
}

// ─── /proc/uptime ────────────────────────────────────────────────────────────
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_uptime(info: &mut SysInfo) {
    let mut buf = [0u8; 64];
    let n = read_file("/proc/uptime", &mut buf);
    // Format: "12345.67 9876.54\n"  (uptime_secs idle_secs)
    if let Some(secs) = parse_float_u64(&buf[..n]) {
        info.uptime_secs = secs;
    }
}

// ─── /proc/loadavg ───────────────────────────────────────────────────────────
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_loadavg(info: &mut SysInfo) {
    let mut buf = [0u8; 64];
    let n = read_file("/proc/loadavg", &mut buf);
    // Format: "0.52 0.48 0.41 2/543 12345"
    let data = &buf[..n];
    let mut parts = data.split(|&b| b == b' ');
    info.load_1 = parse_f32(parts.next().unwrap_or(b""));
    info.load_5 = parse_f32(parts.next().unwrap_or(b""));
    info.load_15 = parse_f32(parts.next().unwrap_or(b""));
}

// ─── hostname ────────────────────────────────────────────────────────────────
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_hostname(info: &mut SysInfo) {
    let mut buf = [0u8; 64];
    let n = read_file("/proc/sys/kernel/hostname", &mut buf);
    let trimmed = trim_bytes(&buf[..n]);
    let len = trimmed.len().min(info.hostname.len());
    info.hostname[..len].copy_from_slice(&trimmed[..len]);
    info.hostname_len = len;
}

// ─── kernel version ──────────────────────────────────────────────────────────
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn read_kernel(info: &mut SysInfo) {
    if info.kernel_len > 0 {
        return; // Kernel version doesn't change at runtime.
    }
    let mut buf = [0u8; 256];
    let n = read_file("/proc/sys/kernel/osrelease", &mut buf);
    let trimmed = trim_bytes(&buf[..n]);
    let len = trimmed.len().min(info.kernel.len());
    info.kernel[..len].copy_from_slice(&trimmed[..len]);
    info.kernel_len = len;
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Parse the integer seconds portion of "12345.67 ..." without float allocation.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn parse_float_u64(b: &[u8]) -> Option<u64> {
    if b.is_empty() {
        return None;
    }
    let mut n = 0u64;
    for &c in b {
        if c == b'.' || c == b' ' || c == b'\n' {
            break;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.wrapping_mul(10).wrapping_add((c - b'0') as u64);
    }
    Some(n)
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn parse_f32(b: &[u8]) -> f32 {
    core::str::from_utf8(b)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.0)
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
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

/// Format uptime seconds into "Nd Hh Mm Ss" in the provided buffer.
/// Returns the written byte count.
pub fn format_uptime(secs: u64, buf: &mut [u8]) -> usize {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    let formatted = if days > 0 {
        format!("{}d {:02}h {:02}m {:02}s", days, hours, mins, s)
    } else if hours > 0 {
        format!("{:02}h {:02}m {:02}s", hours, mins, s)
    } else {
        format!("{:02}m {:02}s", mins, s)
    };
    let bytes = formatted.as_bytes();
    let len = bytes.len().min(buf.len());
    buf[..len].copy_from_slice(&bytes[..len]);
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    #[test]
    fn test_parse_float_u64() {
        assert_eq!(parse_float_u64(b"12345.67 9876.54\n"), Some(12345));
        assert_eq!(parse_float_u64(b"0.0"), Some(0));
        assert_eq!(parse_float_u64(b""), None);
    }

    #[test]
    fn test_format_uptime() {
        let mut buf = [0u8; 64];
        let n = format_uptime(90061, &mut buf);
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "1d 01h 01m 01s");
    }

    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    #[test]
    fn test_parse_loadavg() {
        let data = b"0.52 0.48 0.41 2/543 12345\n";
        let mut info = SysInfo::default();
        let mut parts = data.split(|&b| b == b' ');
        info.load_1 = parse_f32(parts.next().unwrap_or(b""));
        info.load_5 = parse_f32(parts.next().unwrap_or(b""));
        assert!((info.load_1 - 0.52).abs() < 0.001);
        assert!((info.load_5 - 0.48).abs() < 0.001);
    }
}
