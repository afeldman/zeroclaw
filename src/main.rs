//! ZeroClaw — ultra-lightweight Linux system monitor.
//!
//! Entry point: parse args, load config, then run the appropriate display loop.
//!
//! Design principles:
//! - All /proc reads use fixed-size stack buffers (no heap in hot path)
//! - Synchronous I/O only — no async runtime overhead
//! - Single binary, zero required external dependencies at runtime

mod config;
mod cpu;
mod disk;
mod display;
mod mem;
mod net;
mod proc;
mod sysinfo;
mod types;

#[cfg(feature = "mcp")]
mod mcp;

#[cfg(feature = "nvidia")]
mod gpu_nvidia;

#[cfg(feature = "metal")]
mod gpu_metal;

use display::{CLEAR_SCREEN, HIDE_CURSOR, MOVE_HOME, SHOW_CURSOR};
use types::{Args, CpuStats, DiskStat, MemStats, MountInfo, NetInterface, OutputMode, ProcessInfo, SysInfo};

#[cfg(any(feature = "nvidia", feature = "metal"))]
use types::GpuStats;

fn main() {
    let args = parse_args();

    // MCP server mode — run standalone server and exit
    #[cfg(feature = "mcp")]
    if args.mcp_server {
        mcp::run_mcp_server();
        return;
    }

    let mut cfg = config::load();

    // CLI args override config file values.
    if args.interval_secs > 0 { cfg.interval_secs = args.interval_secs; }
    if args.top_n > 0 { cfg.top_n = args.top_n; }
    if args.no_color { cfg.color = false; }

    // Propagate --*-only flags to cfg.show_*
    if args.cpu_only || args.mem_only || args.net_only || args.disk_only || args.proc_only {
        cfg.show_cpu = args.cpu_only;
        cfg.show_memory = args.mem_only;
        cfg.show_network = args.net_only;
        cfg.show_disk = args.disk_only;
        cfg.show_processes = args.proc_only;
    }

    proc::init();

    // Initialize GPU monitoring if enabled
    #[cfg(any(feature = "nvidia", feature = "metal"))]
    let mut gpu_stats = GpuStats::default();
    
    #[cfg(feature = "nvidia")]
    gpu_nvidia::init(&mut gpu_stats);
    
    #[cfg(feature = "metal")]
    gpu_metal::init(&mut gpu_stats);

    match args.mode {
        OutputMode::Once => run_once(&args, &cfg),
        OutputMode::Json => run_json(&args, &cfg),
        OutputMode::Compact => run_compact(&args, &cfg),
        OutputMode::Watch => run_watch(&args, &cfg),
    }
}

// ─── Collection state ─────────────────────────────────────────────────────────

struct State {
    cpu: CpuStats,
    mem: MemStats,
    procs: Vec<ProcessInfo>,
    ifaces: Vec<NetInterface>,
    disks: Vec<DiskStat>,
    mounts: Vec<MountInfo>,
    sys: SysInfo,
    #[cfg(any(feature = "nvidia", feature = "metal"))]
    gpu: GpuStats,
    last_tick: std::time::Instant,
}

impl State {
    fn new() -> Self {
        Self {
            cpu: CpuStats::default(),
            mem: MemStats::default(),
            procs: Vec::new(),
            ifaces: Vec::new(),
            disks: Vec::new(),
            mounts: Vec::new(),
            sys: SysInfo::default(),
            #[cfg(any(feature = "nvidia", feature = "metal"))]
            gpu: GpuStats::default(),
            last_tick: std::time::Instant::now(),
        }
    }

    /// Collect all metrics once.
    fn collect(&mut self, cfg: &types::Config, filter_pid: Option<u32>) {
        let elapsed = self.last_tick.elapsed().as_secs_f32().max(0.01);
        self.last_tick = std::time::Instant::now();

        if cfg.show_cpu { cpu::update(&mut self.cpu); }
        if cfg.show_memory { mem::update(&mut self.mem); }
        if cfg.show_processes || cfg.show_memory {
            proc::update(
                &mut self.procs,
                self.mem.total_kb,
                elapsed,
                cfg.top_n,
                filter_pid,
            );
        }
        if cfg.show_network { net::update(&mut self.ifaces, elapsed); }
        if cfg.show_disk {
            disk::update_io(&mut self.disks, elapsed);
            disk::update_mounts(&mut self.mounts);
        }
        sysinfo::update(&mut self.sys);
        
        // Update GPU stats
        #[cfg(feature = "nvidia")]
        gpu_nvidia::update(&mut self.gpu);
        
        #[cfg(feature = "metal")]
        gpu_metal::update(&mut self.gpu);
    }
}

// ─── Run modes ────────────────────────────────────────────────────────────────

fn run_once(args: &Args, cfg: &types::Config) {
    let mut state = State::new();
    // First pass: initialise CPU delta counters.
    cpu::update(&mut state.cpu);
    std::thread::sleep(std::time::Duration::from_millis(200));
    state.collect(cfg, args.pid);

    let width = display::terminal_width();
    let mut out = display::OutBuf::new(cfg.color);

    render_all(&mut out, &state, cfg, width);
    out.flush();
}

fn run_json(args: &Args, cfg: &types::Config) {
    let mut state = State::new();
    cpu::update(&mut state.cpu);
    std::thread::sleep(std::time::Duration::from_millis(200));

    loop {
        state.collect(cfg, args.pid);
        let mut out = display::OutBuf::new(false);
        display::render_json(
            &mut out,
            &state.cpu,
            &state.mem,
            &state.procs,
            &state.ifaces,
            &state.disks,
            &state.mounts,
            &state.sys,
        );
        out.flush();

        if matches!(args.mode, OutputMode::Once) { break; }
        std::thread::sleep(std::time::Duration::from_secs(cfg.interval_secs as u64));
    }
}

fn run_compact(args: &Args, cfg: &types::Config) {
    let mut state = State::new();
    cpu::update(&mut state.cpu);
    std::thread::sleep(std::time::Duration::from_millis(200));

    loop {
        state.collect(cfg, args.pid);
        let mut out = display::OutBuf::new(false);
        display::render_compact(&mut out, &state.cpu, &state.mem, &state.sys);
        out.flush();

        if matches!(args.mode, OutputMode::Once) { break; }
        std::thread::sleep(std::time::Duration::from_secs(cfg.interval_secs as u64));
    }
}

fn run_watch(args: &Args, cfg: &types::Config) {
    // Enable raw mode and hide cursor.
    let old_termios = raw_mode_enter();
    print!("{}{}", HIDE_CURSOR, CLEAR_SCREEN);

    let mut state = State::new();
    // Seed CPU delta counters before first display.
    cpu::update(&mut state.cpu);
    std::thread::sleep(std::time::Duration::from_millis(500));

    loop {
        state.collect(cfg, args.pid);
        let width = display::terminal_width();
        let mut out = display::OutBuf::new(cfg.color);
        out.push_str(MOVE_HOME);
        render_all(&mut out, &state, cfg, width);
        out.flush();

        // Non-blocking keyboard check: 'q' to quit.
        if key_pressed_q() { break; }
        std::thread::sleep(std::time::Duration::from_secs(cfg.interval_secs as u64));
    }

    raw_mode_restore(old_termios);
    print!("{}", SHOW_CURSOR);
}

fn render_all(
    out: &mut display::OutBuf,
    state: &State,
    cfg: &types::Config,
    width: u16,
) {
    display::render_header(out, &state.sys, width);
    if cfg.show_cpu { display::render_cpu(out, &state.cpu, width, cfg.show_temps); }
    if cfg.show_memory { display::render_mem(out, &state.mem, width); }
    #[cfg(any(feature = "nvidia", feature = "metal"))]
    { display::render_gpu(out, &state.gpu, width); }
    if cfg.show_processes { display::render_procs(out, &state.procs, width); }
    if cfg.show_network { display::render_net(out, &state.ifaces, width); }
    if cfg.show_disk {
        display::render_disk_io(out, &state.disks, width);
        display::render_mounts(out, &state.mounts, width);
    }
}

// ─── Terminal raw mode ────────────────────────────────────────────────────────

fn raw_mode_enter() -> libc::termios {
    let mut old: libc::termios = unsafe { core::mem::zeroed() };
    unsafe {
        libc::tcgetattr(libc::STDIN_FILENO, &mut old);
        let mut raw = old;
        // ECHO off, canonical mode off, signals off.
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        raw.c_cc[libc::VMIN] = 0;  // non-blocking read
        raw.c_cc[libc::VTIME] = 0;
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw);
    }
    old
}

fn raw_mode_restore(old: libc::termios) {
    unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &old); }
}

/// Non-blocking check whether 'q' or Ctrl-C was pressed.
fn key_pressed_q() -> bool {
    let mut buf = [0u8; 1];
    let n = unsafe {
        libc::read(libc::STDIN_FILENO, buf.as_mut_ptr() as *mut libc::c_void, 1)
    };
    n == 1 && (buf[0] == b'q' || buf[0] == 3)
}

// ─── Argument parser ──────────────────────────────────────────────────────────

/// Minimal argument parser — no dependency on clap or any other crate.
/// Under 80 lines of straightforward matching.
fn parse_args() -> Args {
    let mut args = Args::default();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;

    while i < raw.len() {
        match raw[i].as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("zeroclaw {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--once" => args.mode = OutputMode::Once,
            "--json" => args.mode = OutputMode::Json,
            "--compact" => args.mode = OutputMode::Compact,
            "--watch" => args.mode = OutputMode::Watch,
            #[cfg(feature = "mcp")]
            "--mcp" => args.mcp_server = true,
            #[cfg(not(feature = "mcp"))]
            "--mcp" => {
                eprintln!("error: MCP support not compiled in. Rebuild with --features mcp");
                std::process::exit(1);
            }
            "--no-color" | "--no-colour" => args.no_color = true,
            "--cpu-only" => args.cpu_only = true,
            "--mem-only" => args.mem_only = true,
            "--net-only" => args.net_only = true,
            "--disk-only" => args.disk_only = true,
            "--proc-only" => args.proc_only = true,
            "--interval" | "-i" => {
                i += 1;
                if let Some(v) = raw.get(i).and_then(|s| s.parse().ok()) {
                    args.interval_secs = v;
                } else {
                    eprintln!("error: --interval requires a positive integer");
                    std::process::exit(1);
                }
            }
            "--top" | "-n" => {
                i += 1;
                if let Some(v) = raw.get(i).and_then(|s| s.parse().ok()) {
                    args.top_n = v;
                } else {
                    eprintln!("error: --top requires a positive integer");
                    std::process::exit(1);
                }
            }
            "--pid" => {
                i += 1;
                if let Some(v) = raw.get(i).and_then(|s| s.parse().ok()) {
                    args.pid = Some(v);
                } else {
                    eprintln!("error: --pid requires a valid PID");
                    std::process::exit(1);
                }
            }
            unknown => {
                eprintln!("error: unknown argument '{}'\nTry --help.", unknown);
                std::process::exit(1);
            }
        }
        i += 1;
    }
    args
}

#[inline(never)]
fn print_help() {
    print!(
        "zeroclaw {} — ultra-lightweight Linux system monitor

USAGE:
  zeroclaw [OPTIONS]

OPTIONS:
  --once              Print stats once and exit
  --watch             Live-update terminal display (default)
  --json              Output JSON (use with --once or --interval)
  --compact           Single summary line per tick
  --interval, -i N    Refresh every N seconds (default: 1)
  --top, -n N         Show top N processes (default: 10)
  --pid PID           Monitor a single process by PID
  --cpu-only          Show CPU only
  --mem-only          Show memory only
  --net-only          Show network only
  --disk-only         Show disk only
  --proc-only         Show processes only
  --no-color          Disable ANSI colours
  --mcp               Run as MCP server (requires --features mcp)
  --version, -V       Print version and exit
  --help, -h          Print this help

CONFIG:
  ~/.config/zeroclaw/config.toml  (optional)
",
        env!("CARGO_PKG_VERSION")
    );
}
