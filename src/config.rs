//! Hand-written TOML config parser for ~/.config/zeroclaw/config.toml.
//!
//! We parse only the subset of TOML we actually need (<100 lines), avoiding the
//! serde + toml crate dependency that would add ~500 KB to the binary.
//!
//! Supported value types: bare integers, bare floats, bare booleans.
//! Strings (quoted) are not needed for our keys.

use crate::types::Config;
use std::fs::File;
use std::io::Read;

/// Read config from the canonical path.  Returns `Config::default()` if the
/// file does not exist or cannot be parsed.
pub fn load() -> Config {
    let mut cfg = Config::default();

    let home = match std::env::var_os("HOME") {
        Some(h) => h,
        None => return cfg,
    };

    let path = format!("{}/.config/zeroclaw/config.toml", home.to_string_lossy());

    let mut buf = [0u8; 4096];
    let n = {
        let Ok(mut f) = File::open(&path) else {
            return cfg;
        };
        f.read(&mut buf).unwrap_or(0)
    };

    parse_toml(&buf[..n], &mut cfg);
    cfg
}

/// Parse the minimal TOML subset we support.
///
/// Grammar (simplified):
/// ```
/// file   = (line '\n')*
/// line   = comment | section | keyval | empty
/// section = '[' name ']'
/// keyval  = key '=' value
/// comment = '#' ...
/// ```
fn parse_toml(data: &[u8], cfg: &mut Config) {
    let mut section = b"" as &[u8];
    // We need a longer-lived buffer for section names.
    let mut section_buf = [0u8; 32];

    for line in data.split(|&b| b == b'\n') {
        let line = trim(line);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        if line[0] == b'[' {
            // Section header: [display]
            let end = line.iter().position(|&b| b == b']').unwrap_or(line.len());
            let name = trim(&line[1..end]);
            let copy_len = name.len().min(section_buf.len());
            section_buf[..copy_len].copy_from_slice(&name[..copy_len]);
            section = &section_buf[..copy_len];
            continue;
        }
        // key = value
        let Some(eq) = line.iter().position(|&b| b == b'=') else {
            continue;
        };
        let key = trim(&line[..eq]);
        let val = trim(&line[eq + 1..]);
        // Strip inline comment
        let val = if let Some(p) = val.iter().position(|&b| b == b'#') {
            trim(&val[..p])
        } else {
            val
        };

        apply_key(cfg, section, key, val);
    }
}

fn apply_key(cfg: &mut Config, section: &[u8], key: &[u8], val: &[u8]) {
    match section {
        b"display" => match key {
            b"interval" => {
                if let Some(v) = parse_u32(val) {
                    cfg.interval_secs = v.max(1);
                }
            }
            b"top_processes" => {
                if let Some(v) = parse_u32(val) {
                    cfg.top_n = v as usize;
                }
            }
            b"color" => {
                cfg.color = parse_bool(val).unwrap_or(cfg.color);
            }
            _ => {}
        },
        b"features" => match key {
            b"show_cpu" => {
                cfg.show_cpu = parse_bool(val).unwrap_or(cfg.show_cpu);
            }
            b"show_memory" => {
                cfg.show_memory = parse_bool(val).unwrap_or(cfg.show_memory);
            }
            b"show_network" => {
                cfg.show_network = parse_bool(val).unwrap_or(cfg.show_network);
            }
            b"show_disk" => {
                cfg.show_disk = parse_bool(val).unwrap_or(cfg.show_disk);
            }
            b"show_processes" => {
                cfg.show_processes = parse_bool(val).unwrap_or(cfg.show_processes);
            }
            b"show_temps" => {
                cfg.show_temps = parse_bool(val).unwrap_or(cfg.show_temps);
            }
            _ => {}
        },
        b"thresholds" => match key {
            b"cpu_warn" => {
                cfg.cpu_warn = parse_f32(val).unwrap_or(cfg.cpu_warn);
            }
            b"cpu_crit" => {
                cfg.cpu_crit = parse_f32(val).unwrap_or(cfg.cpu_crit);
            }
            b"mem_warn" => {
                cfg.mem_warn = parse_f32(val).unwrap_or(cfg.mem_warn);
            }
            b"mem_crit" => {
                cfg.mem_crit = parse_f32(val).unwrap_or(cfg.mem_crit);
            }
            _ => {}
        },
        _ => {}
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn trim(b: &[u8]) -> &[u8] {
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

fn parse_u32(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut n = 0u32;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(n)
}

fn parse_f32(b: &[u8]) -> Option<f32> {
    // We only need simple decimals like "80.0" or "95".
    core::str::from_utf8(b).ok()?.parse().ok()
}

fn parse_bool(b: &[u8]) -> Option<bool> {
    match b {
        b"true" | b"1" | b"yes" => Some(true),
        b"false" | b"0" | b"no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_toml_basic() {
        let toml = b"[display]\ninterval = 2\ntop_processes = 20\ncolor = false\n\
                     [thresholds]\ncpu_warn = 75.0\n";
        let mut cfg = Config::default();
        parse_toml(toml, &mut cfg);
        assert_eq!(cfg.interval_secs, 2);
        assert_eq!(cfg.top_n, 20);
        assert!(!cfg.color);
        assert_eq!(cfg.cpu_warn, 75.0);
    }

    #[test]
    fn test_parse_toml_comments() {
        let toml = b"# full-line comment\n[display]\ninterval = 3 # inline\n";
        let mut cfg = Config::default();
        parse_toml(toml, &mut cfg);
        assert_eq!(cfg.interval_secs, 3);
    }

    #[test]
    fn test_trim() {
        assert_eq!(trim(b"  hello  "), b"hello");
        assert_eq!(trim(b""), b"");
    }
}
