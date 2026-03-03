//! Shared data structures for ZeroClaw.
//!
//! All structs prefer fixed-size stack arrays over heap-allocated Strings.
//! This avoids heap fragmentation in the hot monitoring loop.

/// Per-core CPU snapshot. Frequencies and temperatures are optional
/// because not all kernels/hardware expose them.
#[derive(Clone, Default)]
pub struct CpuCore {
    pub id: u32,
    /// Percentage 0.0–100.0, computed as delta between two /proc/stat reads.
    pub usage: f32,
    /// MHz reported by /sys/devices/system/cpu/cpuN/cpufreq/scaling_cur_freq.
    pub freq_mhz: u32,
    /// Celsius from /sys/class/thermal/thermal_zoneN/temp (millidegrees / 1000).
    pub temp_c: Option<f32>,
    // Raw jiffies from the previous /proc/stat sample used to compute delta.
    #[allow(dead_code)]
    pub prev_total: u64,
    #[allow(dead_code)]
    pub prev_idle: u64,
}

/// Whole-system CPU snapshot including all per-core data.
pub struct CpuStats {
    /// Null-terminated model string from /proc/cpuinfo "model name".
    pub model: [u8; 128],
    pub model_len: usize,
    pub cores: Vec<CpuCore>,
    /// Aggregate usage across all logical cores.
    pub total_usage: f32,
    // Previous aggregate jiffies for total-usage delta.
    pub prev_total: u64,
    pub prev_idle: u64,
}

impl Default for CpuStats {
    fn default() -> Self {
        Self {
            model: [0; 128],
            model_len: 0,
            cores: Vec::new(),
            total_usage: 0.0,
            prev_total: 0,
            prev_idle: 0,
        }
    }
}

/// Memory snapshot from /proc/meminfo (all values in KiB).
#[derive(Default, Clone)]
pub struct MemStats {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    /// total - free - buffers - cached  (mirrors htop's definition)
    pub used_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub swap_used_kb: u64,
}

/// Single process snapshot derived from /proc/[pid]/stat and /proc/[pid]/status.
#[derive(Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    /// comm field from /proc/[pid]/stat, max 15 chars on Linux.
    pub name: [u8; 64],
    pub name_len: usize,
    /// Percentage 0.0–100.0*num_cpus (matches top(1) behaviour).
    pub cpu_usage: f32,
    /// Resident set size in KiB.
    pub mem_kb: u64,
    pub mem_percent: f32,
    /// State character: R, S, D, Z, T …
    pub status: u8,
    // Raw jiffies from previous sample.
    #[allow(dead_code)]
    pub prev_utime: u64,
    #[allow(dead_code)]
    pub prev_stime: u64,
}

impl Default for ProcessInfo {
    fn default() -> Self {
        Self {
            pid: 0,
            name: [0u8; 64],
            name_len: 0,
            cpu_usage: 0.0,
            mem_kb: 0,
            mem_percent: 0.0,
            status: b'?',
            prev_utime: 0,
            prev_stime: 0,
        }
    }
}

impl ProcessInfo {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// Network interface snapshot from /proc/net/dev.
#[derive(Clone, Default)]
pub struct NetInterface {
    pub name: [u8; 32],
    pub name_len: usize,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    /// Bytes/second since last sample.
    pub rx_rate: f64,
    pub tx_rate: f64,
    // Previous sample values for rate computation.
    pub prev_rx_bytes: u64,
    pub prev_tx_bytes: u64,
}

impl NetInterface {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// Disk I/O snapshot from /proc/diskstats.
#[derive(Clone, Default)]
pub struct DiskStat {
    pub name: [u8; 32],
    pub name_len: usize,
    /// Read/write throughput in bytes/second since last sample.
    pub read_rate: f64,
    pub write_rate: f64,
    // Sector counts from previous sample (1 sector = 512 bytes on Linux).
    #[allow(dead_code)]
    pub prev_read_sectors: u64,
    #[allow(dead_code)]
    pub prev_write_sectors: u64,
}

impl DiskStat {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// Filesystem mount-point utilisation (from statfs(2)).
#[derive(Clone)]
pub struct MountInfo {
    pub mountpoint: [u8; 128],
    pub mountpoint_len: usize,
    pub device: [u8; 64],
    pub device_len: usize,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub usage_percent: f32,
}

impl Default for MountInfo {
    fn default() -> Self {
        Self {
            mountpoint: [0u8; 128],
            mountpoint_len: 0,
            device: [0u8; 64],
            device_len: 0,
            total_bytes: 0,
            free_bytes: 0,
            used_bytes: 0,
            usage_percent: 0.0,
        }
    }
}

impl MountInfo {
    pub fn mount_str(&self) -> &str {
        core::str::from_utf8(&self.mountpoint[..self.mountpoint_len]).unwrap_or("?")
    }
    pub fn device_str(&self) -> &str {
        core::str::from_utf8(&self.device[..self.device_len]).unwrap_or("?")
    }
}

/// System-wide metadata (uptime, load, hostname, kernel).
pub struct SysInfo {
    pub hostname: [u8; 64],
    pub hostname_len: usize,
    pub kernel: [u8; 128],
    pub kernel_len: usize,
    pub uptime_secs: u64,
    pub load_1: f32,
    pub load_5: f32,
    pub load_15: f32,
}

impl Default for SysInfo {
    fn default() -> Self {
        Self {
            hostname: [0u8; 64],
            hostname_len: 0,
            kernel: [0u8; 128],
            kernel_len: 0,
            uptime_secs: 0,
            load_1: 0.0,
            load_5: 0.0,
            load_15: 0.0,
        }
    }
}

impl SysInfo {
    pub fn hostname_str(&self) -> &str {
        core::str::from_utf8(&self.hostname[..self.hostname_len]).unwrap_or("?")
    }
    pub fn kernel_str(&self) -> &str {
        core::str::from_utf8(&self.kernel[..self.kernel_len]).unwrap_or("?")
    }
}

// =============================================================================
// GPU Statistics (NVIDIA/Metal)
// =============================================================================

/// GPU device information and statistics.
#[cfg(any(feature = "nvidia", all(feature = "metal", target_os = "macos")))]
#[derive(Clone)]
pub struct GpuDevice {
    /// GPU index (0-based).
    pub index: u32,
    /// Device name (e.g., "NVIDIA GeForce RTX 4090" or "Apple M4 Max").
    pub name: [u8; 64],
    pub name_len: usize,
    /// GPU utilization percentage (0.0–100.0).
    pub utilization: f32,
    /// Memory utilization percentage (0.0–100.0).
    pub mem_utilization: f32,
    /// Total VRAM in MiB.
    pub mem_total_mb: u64,
    /// Used VRAM in MiB.
    pub mem_used_mb: u64,
    /// GPU temperature in Celsius (None if unavailable).
    pub temp_c: Option<f32>,
    /// Power draw in Watts (None if unavailable).
    pub power_watts: Option<f32>,
    /// Fan speed percentage (None if unavailable or passive cooling).
    pub fan_percent: Option<u32>,
    /// GPU clock frequency in MHz (None if unavailable).
    pub clock_mhz: Option<u32>,
    /// Memory clock frequency in MHz (None if unavailable).
    pub mem_clock_mhz: Option<u32>,
}

#[cfg(any(feature = "nvidia", all(feature = "metal", target_os = "macos")))]
impl Default for GpuDevice {
    fn default() -> Self {
        Self {
            index: 0,
            name: [0u8; 64],
            name_len: 0,
            utilization: 0.0,
            mem_utilization: 0.0,
            mem_total_mb: 0,
            mem_used_mb: 0,
            temp_c: None,
            power_watts: None,
            fan_percent: None,
            clock_mhz: None,
            mem_clock_mhz: None,
        }
    }
}

#[cfg(any(feature = "nvidia", all(feature = "metal", target_os = "macos")))]
impl GpuDevice {
    #[allow(dead_code)]
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// Collection of all GPU devices.
#[cfg(any(feature = "nvidia", all(feature = "metal", target_os = "macos")))]
#[derive(Default)]
pub struct GpuStats {
    pub devices: Vec<GpuDevice>,
    /// True if the GPU library was successfully loaded.
    pub available: bool,
    /// Error message if GPU monitoring failed to initialize.
    pub error: Option<[u8; 128]>,
    pub error_len: usize,
}

#[cfg(any(feature = "nvidia", all(feature = "metal", target_os = "macos")))]
impl GpuStats {
    pub fn error_str(&self) -> Option<&str> {
        if self.error_len > 0 {
            self.error
                .as_ref()
                .and_then(|e| core::str::from_utf8(&e[..self.error_len]).ok())
        } else {
            None
        }
    }
}

/// User-visible output mode.
pub enum OutputMode {
    /// Continuously refresh the terminal (raw mode, ANSI cursor control).
    Watch,
    /// Print once and exit.
    Once,
    /// Emit a single JSON object and exit (or repeat with interval).
    Json,
    /// Single summary line per interval.
    Compact,
}

/// Parsed command-line arguments.
pub struct Args {
    pub mode: OutputMode,
    pub interval_secs: u32,
    pub top_n: usize,
    pub pid: Option<u32>,
    pub cpu_only: bool,
    pub mem_only: bool,
    pub net_only: bool,
    pub disk_only: bool,
    pub proc_only: bool,
    pub no_color: bool,
    /// Run as MCP (Model Context Protocol) server for LLM integration.
    #[cfg(feature = "mcp")]
    pub mcp_server: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            mode: OutputMode::Watch,
            interval_secs: 1,
            top_n: 10,
            pid: None,
            cpu_only: false,
            mem_only: false,
            net_only: false,
            disk_only: false,
            proc_only: false,
            no_color: false,
            #[cfg(feature = "mcp")]
            mcp_server: false,
        }
    }
}

/// Runtime configuration (from ~/.config/zeroclaw/config.toml or defaults).
pub struct Config {
    pub interval_secs: u32,
    pub top_n: usize,
    pub color: bool,
    pub show_cpu: bool,
    pub show_memory: bool,
    pub show_network: bool,
    pub show_disk: bool,
    pub show_processes: bool,
    pub show_temps: bool,
    pub cpu_warn: f32,
    pub cpu_crit: f32,
    pub mem_warn: f32,
    pub mem_crit: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_secs: 1,
            top_n: 10,
            color: true,
            show_cpu: true,
            show_memory: true,
            show_network: true,
            show_disk: true,
            show_processes: true,
            show_temps: true,
            cpu_warn: 80.0,
            cpu_crit: 95.0,
            mem_warn: 80.0,
            mem_crit: 95.0,
        }
    }
}
