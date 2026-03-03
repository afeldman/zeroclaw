//! Network statistics from /proc/net/dev.
//!
//! /proc/net/dev format (after two header lines):
//!   iface: rx_bytes rx_pkts rx_errs rx_drop rx_fifo rx_frame rx_comp rx_mcast
//!          tx_bytes tx_pkts tx_errs tx_drop tx_fifo tx_colls tx_carr tx_comp
//!
//! We compute byte rates by comparing successive readings with the elapsed time.
//!
//! On macOS (with --features macos), uses getifaddrs.

#[cfg(not(feature = "macos"))]
use crate::cpu::read_file;
use crate::types::NetInterface;

/// Update network interface stats.
///
/// `ifaces` persists across calls to maintain previous values for rate calc.
/// `elapsed_secs` — time since the last call.
#[cfg(not(feature = "macos"))]
pub fn update(ifaces: &mut Vec<NetInterface>, elapsed_secs: f32) {
    let mut buf = [0u8; 4096];
    let n = read_file("/proc/net/dev", &mut buf);
    parse_net_dev(&buf[..n], ifaces, elapsed_secs);
}

/// macOS implementation using getifaddrs.
#[cfg(feature = "macos")]
pub fn update(ifaces: &mut Vec<NetInterface>, elapsed_secs: f32) {
    use std::ffi::CStr;
    
    // Store previous values
    let prev: Vec<(String, u64, u64)> = ifaces
        .iter()
        .map(|i| (i.name_str().to_string(), i.rx_bytes, i.tx_bytes))
        .collect();
    ifaces.clear();
    
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    let ret = unsafe { libc::getifaddrs(&mut addrs) };
    if ret != 0 || addrs.is_null() {
        return;
    }
    
    let mut current = addrs;
    while !current.is_null() {
        let entry = unsafe { &*current };
        
        // Only process AF_LINK (datalink) entries which have the stats
        if !entry.ifa_addr.is_null() {
            let family = unsafe { (*entry.ifa_addr).sa_family } as i32;
            
            if family == libc::AF_LINK {
                let name = unsafe { CStr::from_ptr(entry.ifa_name) }
                    .to_str()
                    .unwrap_or("?");
                
                // Skip loopback
                if name != "lo0" {
                    // Get interface data from ifa_data
                    if !entry.ifa_data.is_null() {
                        let data = entry.ifa_data as *const libc::if_data;
                        let if_data = unsafe { &*data };
                        
                        let mut iface = NetInterface::default();
                        let name_bytes = name.as_bytes();
                        let copy_len = name_bytes.len().min(iface.name.len());
                        iface.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
                        iface.name_len = copy_len;
                        
                        iface.rx_bytes = if_data.ifi_ibytes as u64;
                        iface.tx_bytes = if_data.ifi_obytes as u64;
                        iface.rx_packets = if_data.ifi_ipackets as u64;
                        iface.tx_packets = if_data.ifi_opackets as u64;
                        
                        // Find previous values for rate calculation
                        let (prev_rx, prev_tx) = prev
                            .iter()
                            .find(|(n, _, _)| n == name)
                            .map(|(_, rx, tx)| (*rx, *tx))
                            .unwrap_or((iface.rx_bytes, iface.tx_bytes));
                        
                        let dt = elapsed_secs.max(0.001) as f64;
                        iface.rx_rate = iface.rx_bytes.saturating_sub(prev_rx) as f64 / dt;
                        iface.tx_rate = iface.tx_bytes.saturating_sub(prev_tx) as f64 / dt;
                        iface.prev_rx_bytes = iface.rx_bytes;
                        iface.prev_tx_bytes = iface.tx_bytes;
                        
                        ifaces.push(iface);
                    }
                }
            }
        }
        
        current = unsafe { (*current).ifa_next };
    }
    
    unsafe { libc::freeifaddrs(addrs) };
}

#[cfg(not(feature = "macos"))]
fn parse_net_dev(data: &[u8], ifaces: &mut Vec<NetInterface>, elapsed_secs: f32) {
    // Skip the two header lines.
    let mut lines = data.split(|&b| b == b'\n').skip(2);

    // Build lookup of existing interface data (for rate calculation).
    // Manual zero-init because Default is not derived for arrays > 32.
    let mut prev: [([u8; 32], usize, u64, u64); 64] =
        unsafe { core::mem::zeroed() };
    let mut prev_count = 0usize;
    for iface in ifaces.iter() {
        if prev_count < prev.len() {
            prev[prev_count].0[..iface.name_len].copy_from_slice(&iface.name[..iface.name_len]);
            prev[prev_count].1 = iface.name_len;
            prev[prev_count].2 = iface.rx_bytes;
            prev[prev_count].3 = iface.tx_bytes;
            prev_count += 1;
        }
    }
    ifaces.clear();

    for line in lines.by_ref() {
        let line = trim_bytes(line);
        if line.is_empty() { continue; }

        // Find ':' separating interface name from stats.
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let name_bytes = trim_bytes(&line[..colon]);
        if name_bytes.is_empty() { continue; }

        let mut iface = NetInterface::default();
        let copy_len = name_bytes.len().min(iface.name.len());
        iface.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        iface.name_len = copy_len;

        // Parse the 16 numeric fields.
        let rest = &line[colon + 1..];
        let mut nums = [0u64; 16];
        let mut idx = 0usize;
        let mut n = 0u64;
        let mut in_n = false;
        for &b in rest {
            if b.is_ascii_digit() {
                n = n.wrapping_mul(10).wrapping_add((b - b'0') as u64);
                in_n = true;
            } else if in_n {
                if idx < 16 { nums[idx] = n; }
                idx += 1;
                n = 0;
                in_n = false;
                if idx == 16 { break; }
            }
        }
        if in_n && idx < 16 { nums[idx] = n; }

        iface.rx_bytes = nums[0];
        iface.rx_packets = nums[1];
        iface.tx_bytes = nums[8];
        iface.tx_packets = nums[9];

        // Look up previous values.
        let (prev_rx, prev_tx) = find_prev(&prev[..prev_count], &iface.name[..iface.name_len]);
        iface.prev_rx_bytes = iface.rx_bytes;
        iface.prev_tx_bytes = iface.tx_bytes;

        let dt = elapsed_secs.max(0.001) as f64;
        iface.rx_rate = iface.rx_bytes.saturating_sub(prev_rx) as f64 / dt;
        iface.tx_rate = iface.tx_bytes.saturating_sub(prev_tx) as f64 / dt;

        ifaces.push(iface);
    }
}

#[cfg(not(feature = "macos"))]
fn find_prev(prev: &[([u8; 32], usize, u64, u64)], name: &[u8]) -> (u64, u64) {
    for entry in prev {
        if &entry.0[..entry.1] == name {
            return (entry.2, entry.3);
        }
    }
    (0, 0)
}

#[cfg(not(feature = "macos"))]
fn trim_bytes(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|&c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    let end = b.iter().rposition(|&c| !c.is_ascii_whitespace()).map(|p| p + 1).unwrap_or(0);
    if start >= end { b"" } else { &b[start..end] }
}

#[cfg(test)]
#[cfg(not(feature = "macos"))]
mod tests {
    use super::*;

    const MOCK: &[u8] = b"Inter-|   Receive\n \
                           face |bytes    packets\n \
                           lo:    1000    10    0    0    0     0          0         0    1000    10    0    0    0     0       0          0\n \
                           eth0: 99999   500    0    0    0     0          0         0   55555   200    0    0    0     0       0          0\n";

    #[test]
    fn test_parse_net_dev() {
        let mut ifaces = Vec::new();
        parse_net_dev(MOCK, &mut ifaces, 1.0);
        assert_eq!(ifaces.len(), 2);
        let lo = ifaces.iter().find(|i| &i.name[..i.name_len] == b"lo").unwrap();
        assert_eq!(lo.rx_bytes, 1000);
        assert_eq!(lo.tx_bytes, 1000);
        let eth0 = ifaces.iter().find(|i| &i.name[..i.name_len] == b"eth0").unwrap();
        assert_eq!(eth0.rx_bytes, 99999);
    }
}
