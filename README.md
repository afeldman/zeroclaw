# ZeroClaw ⚡

> Ultra-lightweight system monitor written in Rust.  
> Minimal footprint, maximum information — runs on Linux & macOS.

[![CI](https://github.com/afeldman/zeroclaw/actions/workflows/ci.yml/badge.svg)](https://github.com/afeldman/zeroclaw/actions/workflows/ci.yml)
[![Release](https://github.com/afeldman/zeroclaw/actions/workflows/release.yml/badge.svg)](https://github.com/afeldman/zeroclaw/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://rustup.rs)

---

## Features

- **CPU**: per-core usage, frequency, temperature
- **Memory**: RAM and Swap (total/used/free/cached)
- **Processes**: top-N by CPU or RAM, single-PID monitoring
- **Network**: TX/RX bytes/s per interface
- **Disk**: read/write throughput, mountpoint utilisation
- **GPU**: NVIDIA (via NVML) and Apple Metal monitoring
- **System**: uptime, load average, hostname, kernel version
- **MCP Server**: JSON-RPC interface for LLM tool integration
- **Output modes**: interactive watch, one-shot, JSON, compact one-liner

---

## Comparison

| Tool         | Lang     | Binary    | RAM      | Startup   |
|--------------|----------|-----------|----------|-----------|
| htop         | C        | ~680 KB   | ~8 MB    | ~50 ms    |
| btop         | C++      | ~7 MB     | ~30 MB   | ~200 ms   |
| bottom       | Rust     | ~9 MB     | ~15 MB   | ~100 ms   |
| **ZeroClaw** | **Rust** | **<500 KB** | **<5 MB** | **<10 ms** |

---

## Installation

### Pre-built Binaries

Download from [GitHub Releases](https://github.com/afeldman/zeroclaw/releases):

```bash
# Linux x86_64
curl -LO https://github.com/afeldman/zeroclaw/releases/latest/download/zeroclaw-linux-x86_64.tar.gz
tar xzf zeroclaw-linux-x86_64.tar.gz
sudo mv zeroclaw-linux-x86_64 /usr/local/bin/zeroclaw

# Linux x86_64 (static musl, runs everywhere)
curl -LO https://github.com/afeldman/zeroclaw/releases/latest/download/zeroclaw-linux-x86_64-musl.tar.gz

# macOS Apple Silicon
curl -LO https://github.com/afeldman/zeroclaw/releases/latest/download/zeroclaw-macos-aarch64.zip
unzip zeroclaw-macos-aarch64.zip
sudo mv zeroclaw-macos-aarch64 /usr/local/bin/zeroclaw
```

### Build from Source

```bash
git clone https://github.com/afeldman/zeroclaw.git
cd zeroclaw

# Standard release build
make build

# With NVIDIA GPU monitoring (Linux)
make nvidia

# With Metal GPU monitoring (macOS)
make metal

# With MCP server support
make mcp

# Minimal binary (strip + optional UPX)
make small

# Static musl binary (Linux, runs everywhere)
make static

# Install to /usr/local/bin
make install
```

---

## Usage

```
zeroclaw [OPTIONS]

Options:
  --once              Print once and exit
  --watch             Live-update terminal (default)
  --json              JSON output
  --compact           One-line summary per tick
  --mcp               Start MCP server (JSON-RPC over stdio)
  --interval, -i N    Refresh every N seconds (default: 1)
  --top, -n N         Show top N processes (default: 10)
  --pid PID           Monitor a single process
  --cpu-only          Show only CPU stats
  --mem-only          Show only memory stats
  --net-only          Show only network stats
  --disk-only         Show only disk stats
  --proc-only         Show only processes
  --gpu-only          Show only GPU stats
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

# Start MCP server for LLM integration
zeroclaw --mcp
```

---

## Feature Flags

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `mcp`   | MCP JSON-RPC server for LLM tools | tokio, serde, serde_json |
| `macos` | macOS support (sysctl/mach APIs) | — |
| `nvidia`| NVIDIA GPU monitoring via NVML | libnvidia-ml.so (runtime) |
| `metal` | Apple Metal GPU monitoring | IOKit (macOS only) |

Build with features:
```bash
cargo build --release --features mcp,nvidia    # Linux + MCP + NVIDIA
cargo build --release --features macos,metal   # macOS + Metal
```

---

## MCP Server

ZeroClaw can run as an [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, providing system metrics to LLMs:

```bash
zeroclaw --mcp
```

**Available Tools:**
- `get_system_info` — hostname, kernel, uptime, load
- `get_cpu_stats` — CPU usage per core
- `get_memory_stats` — RAM and swap usage
- `get_disk_stats` — disk I/O and mount info
- `get_network_stats` — network interface traffic
- `get_process_list` — top processes by CPU/memory
- `get_gpu_stats` — GPU utilization (if available)

**Integration with Claude:**

Add to your MCP config (`~/.config/claude/mcp.json`):
```json
{
  "mcpServers": {
    "zeroclaw": {
      "command": "/usr/local/bin/zeroclaw",
      "args": ["--mcp"]
    }
  }
}
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
show_gpu = true

[thresholds]
cpu_warn = 80
cpu_crit = 95
mem_warn = 80
mem_crit = 95
```

---

## Why is it so small?

- `opt-level = "z"` + fat LTO + single codegen unit
- `panic = "abort"` removes the unwind machinery (~30 KB)
- No `clap`, `crossterm` — hand-written parsers instead
- Stack-allocated buffers for all `/proc` reads (no heap in hot path)
- Direct `libc` syscalls for terminal and timing
- Optional features: `serde`/`tokio` only when MCP is enabled

---

## Development

```bash
# Run all checks (format + clippy)
make check

# Install pre-commit hook
ln -sf ../../scripts/pre-commit .git/hooks/pre-commit

# Run tests
make test
```

---

## License

MIT — see [LICENSE](LICENSE).
