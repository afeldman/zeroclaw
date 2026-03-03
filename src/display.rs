//! Terminal output: ANSI colour, progress bars, number formatting, JSON.
//!
//! No ncurses, crossterm, or termion dependency.  Raw ANSI escape codes are
//! written to stdout directly via a 32 KB stack-allocated output buffer that
//! is flushed once per frame — a single write(2) syscall per refresh.

use crate::sysinfo::format_uptime;
use crate::types::{CpuStats, DiskStat, MemStats, MountInfo, NetInterface, ProcessInfo, SysInfo};
#[cfg(any(feature = "nvidia", feature = "metal"))]
use crate::types::GpuStats;
use std::io::Write;

// ─── ANSI colour codes ────────────────────────────────────────────────────────

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";
pub const BRIGHT_WHITE: &str = "\x1b[97m";

pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
pub const HIDE_CURSOR: &str = "\x1b[?25l";
pub const SHOW_CURSOR: &str = "\x1b[?25h";
pub const MOVE_HOME: &str = "\x1b[H";

/// Query terminal width via ioctl TIOCGWINSZ. Falls back to 80.
pub fn terminal_width() -> u16 {
    let mut ws: libc::winsize = unsafe { core::mem::zeroed() };
    let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ret == 0 && ws.ws_col > 0 { ws.ws_col } else { 80 }
}

// ─── Output buffer ────────────────────────────────────────────────────────────

/// A stack-allocated write buffer that flushes to stdout in one syscall.
pub struct OutBuf {
    buf: Vec<u8>,
    color: bool,
}

impl OutBuf {
    pub fn new(color: bool) -> Self {
        Self { buf: Vec::with_capacity(32 * 1024), color }
    }

    pub fn push_str(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn push_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn push_color(&mut self, color: &str) {
        if self.color { self.push_str(color); }
    }

    pub fn push_reset(&mut self) {
        if self.color { self.push_str(RESET); }
    }

    /// Flush to stdout.
    pub fn flush(&self) {
        let stdout = std::io::stdout();
        let _ = stdout.lock().write_all(&self.buf);
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

// ─── Section header ────────────────────────────────────────────────────────────

pub fn section(out: &mut OutBuf, title: &str, width: u16) {
    out.push_color(BOLD);
    out.push_color(CYAN);
    out.push_str(title);
    out.push_str(" ");
    let title_len = title.len() + 1;
    let dashes = (width as usize).saturating_sub(title_len);
    for _ in 0..dashes { out.push_str("─"); }
    out.push_reset();
    out.push_str("\n");
}

// ─── CPU ──────────────────────────────────────────────────────────────────────

pub fn render_cpu(out: &mut OutBuf, cpu: &CpuStats, width: u16, show_temps: bool) {
    section(out, "CPU", width);

    // Model line
    if cpu.model_len > 0 {
        out.push_color(DIM);
        out.push_str("  ");
        out.push_bytes(&cpu.model[..cpu.model_len]);
        out.push_reset();
        out.push_str("\n");
    }

    for core in &cpu.cores {
        let label = format!("  Core {:>3}: ", core.id);
        out.push_str(&label);

        bar(out, core.usage, 100.0, 20, core.usage);

        let freq_str = if core.freq_mhz > 0 {
            format!("  {:>4} MHz", core.freq_mhz)
        } else {
            String::new()
        };
        let temp_str = if show_temps {
            core.temp_c.map(|t| format!("  {:>5.1}°C", t)).unwrap_or_default()
        } else {
            String::new()
        };
        out.push_color(DIM);
        out.push_str(&freq_str);
        out.push_str(&temp_str);
        out.push_reset();
        out.push_str("\n");
    }

    // Total line
    out.push_str("  Total:    ");
    bar(out, cpu.total_usage, 100.0, 20, cpu.total_usage);
    out.push_str("\n");
}

// ─── Memory ───────────────────────────────────────────────────────────────────

pub fn render_mem(out: &mut OutBuf, mem: &MemStats, width: u16) {
    section(out, "MEMORY", width);

    let ram_pct = if mem.total_kb > 0 {
        mem.used_kb as f32 / mem.total_kb as f32 * 100.0
    } else { 0.0 };
    let swap_pct = if mem.swap_total_kb > 0 {
        mem.swap_used_kb as f32 / mem.swap_total_kb as f32 * 100.0
    } else { 0.0 };

    out.push_str("  RAM:   ");
    bar(out, ram_pct, 100.0, 28, ram_pct);
    let ram_str = format!("  {}/{} GB ({:.0}%)\n",
        fmt_gib(mem.used_kb * 1024),
        fmt_gib(mem.total_kb * 1024),
        ram_pct);
    out.push_str(&ram_str);

    out.push_str("  Swap:  ");
    bar(out, swap_pct, 100.0, 28, swap_pct);
    let swap_str = format!("  {}/{} GB ({:.0}%)\n",
        fmt_gib(mem.swap_used_kb * 1024),
        fmt_gib(mem.swap_total_kb * 1024),
        swap_pct);
    out.push_str(&swap_str);

    out.push_color(DIM);
    let detail = format!("  Buffers: {}  Cached: {}  Available: {}\n",
        fmt_mib(mem.buffers_kb * 1024),
        fmt_mib(mem.cached_kb * 1024),
        fmt_mib(mem.available_kb * 1024));
    out.push_str(&detail);
    out.push_reset();
}

// ─── Processes ────────────────────────────────────────────────────────────────

pub fn render_procs(out: &mut OutBuf, procs: &[ProcessInfo], width: u16) {
    section(out, "TOP PROCESSES", width);
    out.push_color(BOLD);
    out.push_str(&format!(
        "  {:>6}  {:<20}  {:>6}  {:>6}  {}\n",
        "PID", "NAME", "CPU%", "MEM%", "STATUS"
    ));
    out.push_reset();

    for p in procs {
        let state = p.status as char;
        let state_color = match p.status {
            b'R' => GREEN,
            b'D' => RED,
            b'Z' => RED,
            _ => WHITE,
        };

        out.push_str(&format!(
            "  {:>6}  {:<20}  {:>5.1}%  {:>5.1}%  ",
            p.pid,
            p.name_str(),
            p.cpu_usage,
            p.mem_percent
        ));
        out.push_color(state_color);
        out.push_str(&format!("{}\n", state));
        out.push_reset();
    }
}

// ─── Network ──────────────────────────────────────────────────────────────────

pub fn render_net(out: &mut OutBuf, ifaces: &[NetInterface], width: u16) {
    section(out, "NETWORK", width);
    for iface in ifaces {
        out.push_str(&format!(
            "  {:>12}:  ↓ {:>10}/s   ↑ {:>10}/s\n",
            iface.name_str(),
            fmt_bytes_rate(iface.rx_rate),
            fmt_bytes_rate(iface.tx_rate),
        ));
    }
}

// ─── Disk ─────────────────────────────────────────────────────────────────────

pub fn render_disk_io(out: &mut OutBuf, disks: &[DiskStat], width: u16) {
    if disks.is_empty() { return; }
    section(out, "DISK I/O", width);
    for d in disks {
        out.push_str(&format!(
            "  {:>8}:  R {:>10}/s   W {:>10}/s\n",
            d.name_str(),
            fmt_bytes_rate(d.read_rate),
            fmt_bytes_rate(d.write_rate),
        ));
    }
}

pub fn render_mounts(out: &mut OutBuf, mounts: &[MountInfo], width: u16) {
    if mounts.is_empty() { return; }
    section(out, "FILESYSTEMS", width);
    for m in mounts {
        let pct = m.usage_percent;
        out.push_str("  ");
        out.push_str(&format!("{:<16}  ", m.mount_str()));
        bar(out, pct, 100.0, 20, pct);
        out.push_str(&format!(
            "  {} / {}  ({:.0}%)\n",
            fmt_gib(m.used_bytes),
            fmt_gib(m.total_bytes),
            pct
        ));
    }
}

// ─── System info header ───────────────────────────────────────────────────────

pub fn render_header(out: &mut OutBuf, sys: &SysInfo, width: u16) {
    let mut uptime_buf = [0u8; 32];
    let n = format_uptime(sys.uptime_secs, &mut uptime_buf);
    let uptime_str = core::str::from_utf8(&uptime_buf[..n]).unwrap_or("?");

    out.push_color(BOLD);
    out.push_color(BRIGHT_WHITE);
    let title = format!(
        "ZeroClaw  ─  {}  ─  up {}  ─  load {:.2}/{:.2}/{:.2}",
        sys.hostname_str(),
        uptime_str,
        sys.load_1, sys.load_5, sys.load_15
    );
    out.push_str(&title);
    let pad = (width as usize).saturating_sub(title.len());
    for _ in 0..pad { out.push_str(" "); }
    out.push_reset();
    out.push_str("\n");
    out.push_color(DIM);
    out.push_str(&format!("  kernel: {}\n\n", sys.kernel_str()));
    out.push_reset();
}

// ─── Compact one-liner ────────────────────────────────────────────────────────

pub fn render_compact(out: &mut OutBuf, cpu: &CpuStats, mem: &MemStats, sys: &SysInfo) {
    let ram_pct = if mem.total_kb > 0 {
        mem.used_kb as f32 / mem.total_kb as f32 * 100.0
    } else { 0.0 };
    let line = format!(
        "cpu:{:.1}%  mem:{:.1}%  load:{:.2}  up:{}s\n",
        cpu.total_usage,
        ram_pct,
        sys.load_1,
        sys.uptime_secs
    );
    out.push_str(&line);
}

// ─── JSON output ──────────────────────────────────────────────────────────────

pub fn render_json(
    out: &mut OutBuf,
    cpu: &CpuStats,
    mem: &MemStats,
    procs: &[ProcessInfo],
    ifaces: &[NetInterface],
    disks: &[DiskStat],
    mounts: &[MountInfo],
    sys: &SysInfo,
) {
    let ts = unix_timestamp();
    out.push_str("{\n");
    out.push_str(&format!("  \"timestamp\": {},\n", ts));
    out.push_str(&format!("  \"hostname\": \"{}\",\n", sys.hostname_str()));
    out.push_str(&format!("  \"kernel\": \"{}\",\n", sys.kernel_str()));
    out.push_str(&format!("  \"uptime_secs\": {},\n", sys.uptime_secs));
    out.push_str(&format!(
        "  \"load\": {{\"1m\": {:.2}, \"5m\": {:.2}, \"15m\": {:.2}}},\n",
        sys.load_1, sys.load_5, sys.load_15
    ));

    // CPU
    out.push_str("  \"cpu\": {\n");
    out.push_str(&format!(
        "    \"model\": \"{}\",\n",
        core::str::from_utf8(&cpu.model[..cpu.model_len]).unwrap_or("")
    ));
    out.push_str(&format!("    \"total_usage\": {:.1},\n", cpu.total_usage));
    out.push_str("    \"cores\": [\n");
    for (i, c) in cpu.cores.iter().enumerate() {
        let comma = if i + 1 < cpu.cores.len() { "," } else { "" };
        let temp = c.temp_c.map(|t| format!(", \"temp_c\": {:.1}", t)).unwrap_or_default();
        out.push_str(&format!(
            "      {{\"id\": {}, \"usage\": {:.1}, \"freq_mhz\": {}{}}}{}",
            c.id, c.usage, c.freq_mhz, temp, comma
        ));
        out.push_str("\n");
    }
    out.push_str("    ]\n  },\n");

    // Memory
    out.push_str("  \"memory\": {\n");
    out.push_str(&format!("    \"total_kb\": {},\n", mem.total_kb));
    out.push_str(&format!("    \"used_kb\": {},\n", mem.used_kb));
    out.push_str(&format!("    \"free_kb\": {},\n", mem.free_kb));
    out.push_str(&format!("    \"available_kb\": {},\n", mem.available_kb));
    out.push_str(&format!("    \"cached_kb\": {},\n", mem.cached_kb));
    out.push_str(&format!("    \"swap_total_kb\": {},\n", mem.swap_total_kb));
    out.push_str(&format!("    \"swap_used_kb\": {}\n", mem.swap_used_kb));
    out.push_str("  },\n");

    // Processes
    out.push_str("  \"processes\": [\n");
    for (i, p) in procs.iter().enumerate() {
        let comma = if i + 1 < procs.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"pid\": {}, \"name\": \"{}\", \"cpu\": {:.1}, \"mem_kb\": {}}}{}",
            p.pid, p.name_str(), p.cpu_usage, p.mem_kb, comma
        ));
        out.push_str("\n");
    }
    out.push_str("  ],\n");

    // Network
    out.push_str("  \"network\": [\n");
    for (i, iface) in ifaces.iter().enumerate() {
        let comma = if i + 1 < ifaces.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"name\": \"{}\", \"rx_bytes\": {}, \"tx_bytes\": {}, \
             \"rx_rate\": {:.0}, \"tx_rate\": {:.0}}}{}",
            iface.name_str(), iface.rx_bytes, iface.tx_bytes,
            iface.rx_rate, iface.tx_rate, comma
        ));
        out.push_str("\n");
    }
    out.push_str("  ],\n");

    // Disks
    out.push_str("  \"disks\": [\n");
    for (i, d) in disks.iter().enumerate() {
        let comma = if i + 1 < disks.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"name\": \"{}\", \"read_rate\": {:.0}, \"write_rate\": {:.0}}}{}",
            d.name_str(), d.read_rate, d.write_rate, comma
        ));
        out.push_str("\n");
    }
    out.push_str("  ],\n");

    // Mounts
    out.push_str("  \"mounts\": [\n");
    for (i, m) in mounts.iter().enumerate() {
        let comma = if i + 1 < mounts.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"mountpoint\": \"{}\", \"device\": \"{}\", \
             \"total_bytes\": {}, \"used_bytes\": {}, \"usage_pct\": {:.1}}}{}",
            m.mount_str(), m.device_str(), m.total_bytes, m.used_bytes, m.usage_percent, comma
        ));
        out.push_str("\n");
    }
    out.push_str("  ]\n}\n");
}

// ─── Progress bar ─────────────────────────────────────────────────────────────

/// Render a coloured ASCII progress bar into `out`.
/// `value` and `max` determine fill fraction; `threshold` picks the colour.
fn bar(out: &mut OutBuf, value: f32, max: f32, width: usize, threshold: f32) {
    let filled = if max > 0.0 {
        ((value / max).clamp(0.0, 1.0) * width as f32) as usize
    } else {
        0
    };
    let color = if threshold >= 95.0 {
        RED
    } else if threshold >= 80.0 {
        YELLOW
    } else {
        GREEN
    };

    out.push_str("[");
    out.push_color(color);
    for _ in 0..filled { out.push_str("█"); }
    out.push_reset();
    out.push_color(DIM);
    for _ in filled..width { out.push_str("░"); }
    out.push_reset();
    out.push_str("]");

    out.push_color(color);
    out.push_str(&format!(" {:>5.1}%", value));
    out.push_reset();
}

// ─── Number formatting ────────────────────────────────────────────────────────

fn fmt_gib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gib >= 1.0 {
        format!("{:.1}G", gib)
    } else {
        format!("{:.0}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn fmt_mib(bytes: u64) -> String {
    format!("{:.0}M", bytes as f64 / (1024.0 * 1024.0))
}

fn fmt_bytes_rate(bps: f64) -> String {
    if bps >= 1_000_000_000.0 {
        format!("{:.1} GB", bps / 1_000_000_000.0)
    } else if bps >= 1_000_000.0 {
        format!("{:.1} MB", bps / 1_000_000.0)
    } else if bps >= 1_000.0 {
        format!("{:.1} KB", bps / 1_000.0)
    } else {
        format!("{:.0}  B", bps)
    }
}

fn unix_timestamp() -> u64 {
    let mut ts: libc::timespec = unsafe { core::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
    ts.tv_sec as u64
}

// ─── GPU ──────────────────────────────────────────────────────────────────────

#[cfg(any(feature = "nvidia", feature = "metal"))]
pub fn render_gpu(out: &mut OutBuf, gpu: &GpuStats, width: u16) {
    if !gpu.available && gpu.devices.is_empty() {
        // Don't render section if GPU monitoring failed or no devices
        if let Some(err) = gpu.error_str() {
            out.push_color(DIM);
            out.push_str("  GPU: ");
            out.push_str(err);
            out.push_str("\n");
            out.push_reset();
        }
        return;
    }
    
    section(out, "GPU", width);
    
    for device in &gpu.devices {
        // Device name
        out.push_color(DIM);
        out.push_str("  ");
        out.push_bytes(&device.name[..device.name_len]);
        out.push_reset();
        out.push_str("\n");
        
        // GPU utilization bar
        out.push_str("    GPU:  ");
        bar(out, device.utilization, 100.0, 20, device.utilization);
        
        // Additional info on same line
        let mut extras = String::new();
        if let Some(temp) = device.temp_c {
            extras.push_str(&format!("  {:>4.0}°C", temp));
        }
        if let Some(power) = device.power_watts {
            extras.push_str(&format!("  {:>5.1}W", power));
        }
        if let Some(clock) = device.clock_mhz {
            extras.push_str(&format!("  {:>4} MHz", clock));
        }
        if !extras.is_empty() {
            out.push_color(DIM);
            out.push_str(&extras);
            out.push_reset();
        }
        out.push_str("\n");
        
        // VRAM bar (if available)
        if device.mem_total_mb > 0 {
            let mem_pct = device.mem_used_mb as f32 / device.mem_total_mb as f32 * 100.0;
            out.push_str("    VRAM: ");
            bar(out, mem_pct, 100.0, 20, mem_pct);
            out.push_str(&format!("  {}/{}M ({:.0}%)\n",
                device.mem_used_mb, device.mem_total_mb, mem_pct));
        }
        
        // Fan speed (if available)
        if let Some(fan) = device.fan_percent {
            out.push_color(DIM);
            out.push_str(&format!("    Fan: {}%", fan));
            out.push_reset();
            out.push_str("\n");
        }
    }
}
