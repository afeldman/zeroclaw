//! Disk I/O statistics from /proc/diskstats and mount utilisation via statfs(2).
//!
//! /proc/diskstats fields (space-separated):
//!   major minor name reads_completed reads_merged sectors_read ms_reading
//!   writes_completed writes_merged sectors_written ms_writing
//!   ios_in_flight ms_doing_io ms_weighted
//!
//! 1 sector = 512 bytes on Linux (hardcoded in the kernel regardless of device
//! physical sector size — the kernel always reports in 512-byte units here).
//!
//! On macOS (with --features macos), I/O stats use IOKit and mounts use getfsstat.

#[cfg(not(all(feature = "macos", target_os = "macos")))]
use crate::cpu::{parse_u64, read_file};
use crate::types::{DiskStat, MountInfo};

#[cfg(not(all(feature = "macos", target_os = "macos")))]
const SECTOR_BYTES: u64 = 512;

/// Update disk I/O rates.
///
/// `disks` persists between calls to maintain previous sector counts.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn update_io(disks: &mut Vec<DiskStat>, elapsed_secs: f32) {
    let mut buf = [0u8; 4096];
    let n = read_file("/proc/diskstats", &mut buf);
    parse_diskstats(&buf[..n], disks, elapsed_secs);
}

/// macOS: Disk I/O stats are complex to get via IOKit, using stub for now.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn update_io(_disks: &mut Vec<DiskStat>, _elapsed_secs: f32) {
    // IOKit disk stats require significant additional code.
    // For now, this is a placeholder. Full implementation would use:
    // - IOServiceMatching("IOBlockStorageDriver")
    // - IOServiceGetMatchingServices
    // - IORegistryEntryCreateCFProperties for "Statistics"
}

/// Refresh mount utilisation via the statfs(2) syscall.
#[cfg(not(all(feature = "macos", target_os = "macos")))]
pub fn update_mounts(mounts: &mut Vec<MountInfo>) {
    mounts.clear();
    let mut buf = [0u8; 4096];
    let n = read_file("/proc/mounts", &mut buf);
    let data = &buf[..n];

    for line in data.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        // Format: device mountpoint fstype options dump pass
        let mut parts = line.split(|&b| b == b' ');
        let device = parts.next().unwrap_or(b"");
        let mountpoint = parts.next().unwrap_or(b"");
        let fstype = parts.next().unwrap_or(b"");

        // Skip pseudo filesystems.
        if matches!(
            fstype,
            b"proc"
                | b"sysfs"
                | b"devtmpfs"
                | b"devpts"
                | b"tmpfs"
                | b"cgroup"
                | b"cgroup2"
                | b"pstore"
                | b"bpf"
                | b"securityfs"
                | b"debugfs"
                | b"tracefs"
                | b"mqueue"
                | b"hugetlbfs"
                | b"fusectl"
                | b"rpc_pipefs"
                | b"none"
        ) {
            continue;
        }

        let mp_str = match core::str::from_utf8(mountpoint) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if let Some(mi) = statfs_mount(mp_str, device, mountpoint) {
            mounts.push(mi);
        }
    }

    // Deduplicate by mountpoint (some mounts appear multiple times).
    mounts.dedup_by(|a, b| a.mountpoint[..a.mountpoint_len] == b.mountpoint[..b.mountpoint_len]);
}

/// macOS implementation using getfsstat.
#[cfg(all(feature = "macos", target_os = "macos"))]
pub fn update_mounts(mounts: &mut Vec<MountInfo>) {
    use std::ffi::CStr;
    use std::mem::MaybeUninit;

    mounts.clear();

    // First call to get count
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        return;
    }

    // Allocate buffer
    let mut stats: Vec<libc::statfs> =
        vec![unsafe { MaybeUninit::zeroed().assume_init() }; count as usize];
    let buf_size = (count as usize) * std::mem::size_of::<libc::statfs>();

    let actual = unsafe { libc::getfsstat(stats.as_mut_ptr(), buf_size as i32, libc::MNT_NOWAIT) };

    if actual <= 0 {
        return;
    }

    for st in stats.iter().take(actual as usize) {
        // Skip pseudo filesystems
        let fstype = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) }
            .to_str()
            .unwrap_or("");

        if fstype == "devfs" || fstype == "autofs" || fstype == "nullfs" {
            continue;
        }

        let mountpoint = unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) }
            .to_str()
            .unwrap_or("");
        let device = unsafe { CStr::from_ptr(st.f_mntfromname.as_ptr()) }
            .to_str()
            .unwrap_or("");

        if st.f_blocks == 0 {
            continue;
        }

        let mut mi = MountInfo::default();

        let mp_bytes = mountpoint.as_bytes();
        let mp_len = mp_bytes.len().min(mi.mountpoint.len() - 1);
        mi.mountpoint[..mp_len].copy_from_slice(&mp_bytes[..mp_len]);
        mi.mountpoint_len = mp_len;

        let dev_bytes = device.as_bytes();
        let dev_len = dev_bytes.len().min(mi.device.len() - 1);
        mi.device[..dev_len].copy_from_slice(&dev_bytes[..dev_len]);
        mi.device_len = dev_len;

        let bsize = st.f_bsize as u64;
        mi.total_bytes = st.f_blocks as u64 * bsize;
        mi.free_bytes = st.f_bavail as u64 * bsize;
        mi.used_bytes = mi.total_bytes.saturating_sub(st.f_bfree as u64 * bsize);
        mi.usage_percent = if mi.total_bytes > 0 {
            mi.used_bytes as f32 / mi.total_bytes as f32 * 100.0
        } else {
            0.0
        };

        mounts.push(mi);
    }
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn statfs_mount(path: &str, device: &[u8], mountpoint: &[u8]) -> Option<MountInfo> {
    use std::ffi::CString;
    let cpath = CString::new(path).ok()?;
    let mut st: libc::statfs = unsafe { core::mem::zeroed() };
    let ret = unsafe { libc::statfs(cpath.as_ptr(), &mut st) };
    if ret != 0 {
        return None;
    }
    if st.f_blocks == 0 {
        return None;
    } // zero-size fs

    let mut mi = MountInfo::default();

    let mp_len = mountpoint.len().min(mi.mountpoint.len() - 1);
    mi.mountpoint[..mp_len].copy_from_slice(&mountpoint[..mp_len]);
    mi.mountpoint_len = mp_len;

    let dev_len = device.len().min(mi.device.len() - 1);
    mi.device[..dev_len].copy_from_slice(&device[..dev_len]);
    mi.device_len = dev_len;

    let bsize = st.f_bsize as u64;
    mi.total_bytes = st.f_blocks * bsize;
    mi.free_bytes = st.f_bavail * bsize; // available to unprivileged users
    mi.used_bytes = mi.total_bytes.saturating_sub(st.f_bfree * bsize);
    mi.usage_percent = if mi.total_bytes > 0 {
        mi.used_bytes as f32 / mi.total_bytes as f32 * 100.0
    } else {
        0.0
    };

    Some(mi)
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn parse_diskstats(data: &[u8], disks: &mut Vec<DiskStat>, elapsed_secs: f32) {
    // Collect previous sector counts.
    // Manual zero-init because Default is not derived for arrays > 32.
    let mut prev: [([u8; 32], usize, u64, u64); 64] = unsafe { core::mem::zeroed() };
    let mut prev_count = 0usize;
    for d in disks.iter() {
        if prev_count < prev.len() {
            prev[prev_count].0[..d.name_len].copy_from_slice(&d.name[..d.name_len]);
            prev[prev_count].1 = d.name_len;
            prev[prev_count].2 = d.prev_read_sectors;
            prev[prev_count].3 = d.prev_write_sectors;
            prev_count += 1;
        }
    }
    disks.clear();

    for line in data.split(|&b| b == b'\n') {
        let line = trim_bytes(line);
        if line.is_empty() {
            continue;
        }

        let mut fields = line.split(|&b| b == b' ').filter(|s| !s.is_empty());
        fields.next(); // major
        fields.next(); // minor
        let name = match fields.next() {
            Some(n) => n,
            None => continue,
        };

        // Skip partition entries (e.g. sda1, sdb2) — only track whole disks.
        // Heuristic: name ends with a digit → partition.
        if name.last().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            // Allow nvme devices like nvme0n1 but skip nvme0n1p1
            let is_nvme_partition = name.windows(2).any(|w| w == b"n1p" || w == b"n2p");
            if !name.starts_with(b"nvme") || is_nvme_partition {
                continue;
            }
        }

        let mut nums = [0u64; 11];
        for (i, f) in fields.take(11).enumerate() {
            nums[i] = parse_u64(f).unwrap_or(0);
        }
        // nums: [reads_completed, reads_merged, sectors_read, ms_reading,
        //        writes_completed, writes_merged, sectors_written, ms_writing,
        //        ios_in_flight, ms_doing_io, ms_weighted]
        let read_sectors = nums[2];
        let write_sectors = nums[6];

        let mut disk = DiskStat::default();
        let copy_len = name.len().min(disk.name.len());
        disk.name[..copy_len].copy_from_slice(&name[..copy_len]);
        disk.name_len = copy_len;
        disk.prev_read_sectors = read_sectors;
        disk.prev_write_sectors = write_sectors;

        let (found, prev_r, prev_w) = find_prev(&prev[..prev_count], &disk.name[..disk.name_len]);
        if found {
            let dt = elapsed_secs.max(0.001) as f64;
            disk.read_rate = read_sectors.saturating_sub(prev_r) as f64 * SECTOR_BYTES as f64 / dt;
            disk.write_rate =
                write_sectors.saturating_sub(prev_w) as f64 * SECTOR_BYTES as f64 / dt;
        }
        // On first call (no previous data) rates remain 0 — counters are seeded above.

        disks.push(disk);
    }
}

#[cfg(not(all(feature = "macos", target_os = "macos")))]
fn find_prev(prev: &[([u8; 32], usize, u64, u64)], name: &[u8]) -> (bool, u64, u64) {
    for entry in prev {
        if &entry.0[..entry.1] == name {
            return (true, entry.2, entry.3);
        }
    }
    (false, 0, 0)
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

#[cfg(test)]
#[cfg(not(all(feature = "macos", target_os = "macos")))]
mod tests {
    use super::*;

    const MOCK_DISKSTATS: &[u8] = b"   8       0 sda 1000 0 50000 100 500 0 20000 80 0 100 180\n\
           8       1 sda1 900 0 48000 90 400 0 18000 70 0 90 160\n";

    #[test]
    fn test_parse_diskstats_skips_partitions() {
        let mut disks = Vec::new();
        parse_diskstats(MOCK_DISKSTATS, &mut disks, 1.0);
        // sda1 should be skipped (partition)
        assert_eq!(disks.len(), 1);
        assert_eq!(&disks[0].name[..disks[0].name_len], b"sda");
        // First call: no previous values, rates should be 0.
        assert_eq!(disks[0].read_rate, 0.0);
    }

    #[test]
    fn test_io_rates() {
        let mut disks = vec![DiskStat {
            name: *b"sda\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            name_len: 3,
            prev_read_sectors: 40000,
            prev_write_sectors: 10000,
            ..Default::default()
        }];
        // Simulate 10 000 new read sectors = 5 MB in 1 second
        let data = b"   8       0 sda 1000 0 50000 100 500 0 20000 80 0 100 180\n";
        parse_diskstats(data, &mut disks, 1.0);
        let d = &disks[0];
        assert!((d.read_rate - (10000.0 * 512.0)).abs() < 1.0);
    }
}
