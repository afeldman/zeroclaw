# ZeroClaw ⚡

> Ultra-lightweight Linux system monitor written in Rust.
> Minimal footprint, maximum information.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://rustup.rs)

---

## Comparison

| Tool       | Lang  | Binary  | RAM    | Startup |
|------------|-------|---------|--------|---------|
| htop       | C     | ~680 KB | ~8 MB  | ~50 ms  |
| btop       | C++   | ~7 MB   | ~30 MB | ~200 ms |
| bottom     | Rust  | ~9 MB   | ~15 MB | ~100 ms |
| **ZeroClaw** | **Rust** | **<2 MB** | **<5 MB** | **<10 ms** |

---

## Features

- **CPU**: per-core usage, frequency, temperature
- **Memory**: RAM and Swap (total/used/free/cached)
- **Processes**: top-N by CPU or RAM, single-PID monitoring
- **Network**: TX/RX bytes/s per interface
- **Disk**: read/write throughput, mountpoint utilisation
- **System**: uptime, load average, hostname, kernel version
- **Output modes**: interactive watch, one-shot, JSON, compact one-liner

---

## Build

```bash
# Standard release build
make build

# Minimal binary (strip + optional UPX)
make small

# Static musl binary (single file, runs everywhere)
make static

# Install to /usr/local/bin
make install
```

---

## Usage

```
zeroclaw [OPTIONS]

--once              Print once and exit
--watch             Live-update terminal (default)
--json              JSON output
--compact           One-line summary per tick
--interval, -i N    Refresh every N seconds (default: 1)
--top, -n N         Show top N processes (default: 10)
--pid PID           Monitor a single process
--cpu-only / --mem-only / --net-only / --disk-only / --proc-only
--no-color          Disable ANSI colours
```

### Examples

```bash
# Interactive monitor
zeroclaw

# One-shot JSON for scripting
zeroclaw --once --json | jq '.cpu.total_usage'

# CPU only, refresh every 2 s
zeroclaw --cpu-only --interval 2

# Watch a specific process
zeroclaw --pid $(pgrep postgres)

# Compact one-liner for status bars
zeroclaw --compact --once
```

---

## Configuration

`~/.config/zeroclaw/config.toml` (all keys optional):

```toml
[display]
interval = 1
top_processes = 10
color = true

[features]
show_cpu = true
show_memory = true
show_network = true
show_disk = true
show_processes = true
show_temps = true

[thresholds]
cpu_warn = 80
cpu_crit = 95
mem_warn = 80
mem_crit = 95
```

---

## Why is it small?

- `opt-level = "z"` + fat LTO + single codegen unit
- `panic = "abort"` removes the unwind machinery (~30 KB)
- No `serde`, `tokio`, `clap`, `crossterm` — hand-written parsers instead
- Stack-allocated buffers for all `/proc` reads (no heap in hot path)
- Direct `libc` syscalls for terminal and timing

---

## License

MIT — see [LICENSE](LICENSE).
