//! MCP (Model Context Protocol) server for ZeroClaw.
//!
//! Enables LLMs to query system metrics via the MCP stdio transport.
//! Compile with `--features mcp` to include this module.
//!
//! # Protocol
//! JSON-RPC 2.0 over stdin/stdout, following the MCP specification.
//!
//! # Available Tools
//! - `get_cpu_stats` — CPU usage per core, model, frequencies, temperatures
//! - `get_memory_stats` — RAM and swap usage
//! - `get_network_stats` — Per-interface RX/TX rates
//! - `get_disk_stats` — I/O rates and mount point usage
//! - `get_process_list` — Top N processes by CPU/memory
//! - `get_system_info` — Hostname, uptime, load averages

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use crate::types::{
    Config, CpuStats, DiskStat, MemStats, MountInfo, NetInterface, ProcessInfo, SysInfo,
};
use crate::{cpu, disk, mem, net, proc, sysinfo};

// ─── JSON-RPC Types ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

// ─── MCP Protocol Types ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct McpServerInfo {
    name: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct McpCapabilities {
    tools: HashMap<String, serde_json::Value>,
}

#[derive(Serialize)]
struct McpInitializeResult {
    #[serde(rename = "protocolVersion")]
    protocol_version: &'static str,
    #[serde(rename = "serverInfo")]
    server_info: McpServerInfo,
    capabilities: McpCapabilities,
}

#[derive(Serialize)]
struct McpTool {
    name: &'static str,
    description: &'static str,
    #[serde(rename = "inputSchema")]
    input_schema: serde_json::Value,
}

#[derive(Serialize)]
struct McpToolsListResult {
    tools: Vec<McpTool>,
}

#[derive(Serialize)]
struct McpToolResult {
    content: Vec<McpContent>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
}

#[derive(Serialize)]
struct McpContent {
    #[serde(rename = "type")]
    content_type: &'static str,
    text: String,
}

// ─── Response Data Types ──────────────────────────────────────────────────────

#[derive(Serialize)]
struct CpuResponse {
    model: String,
    total_usage_percent: f32,
    cores: Vec<CoreInfo>,
}

#[derive(Serialize)]
struct CoreInfo {
    id: u32,
    usage_percent: f32,
    freq_mhz: u32,
    temp_c: Option<f32>,
}

#[derive(Serialize)]
struct MemResponse {
    total_mb: u64,
    used_mb: u64,
    available_mb: u64,
    used_percent: f32,
    swap_total_mb: u64,
    swap_used_mb: u64,
}

#[derive(Serialize)]
struct NetResponse {
    interfaces: Vec<InterfaceInfo>,
}

#[derive(Serialize)]
struct InterfaceInfo {
    name: String,
    rx_bytes_per_sec: u64,
    tx_bytes_per_sec: u64,
    rx_total_mb: u64,
    tx_total_mb: u64,
}

#[derive(Serialize)]
struct DiskResponse {
    io_stats: Vec<DiskIoInfo>,
    mounts: Vec<MountInfoResponse>,
}

#[derive(Serialize)]
struct DiskIoInfo {
    name: String,
    read_kb_per_sec: u64,
    write_kb_per_sec: u64,
}

#[derive(Serialize)]
struct MountInfoResponse {
    device: String,
    mount_point: String,
    total_gb: f64,
    used_gb: f64,
    available_gb: f64,
    used_percent: f32,
}

#[derive(Serialize)]
struct ProcessResponse {
    processes: Vec<ProcessInfoResponse>,
}

#[derive(Serialize)]
struct ProcessInfoResponse {
    pid: u32,
    name: String,
    cpu_percent: f32,
    mem_mb: u64,
    mem_percent: f32,
    status: String,
}

#[derive(Serialize)]
struct SystemInfoResponse {
    hostname: String,
    kernel: String,
    uptime_secs: u64,
    load_1: f32,
    load_5: f32,
    load_15: f32,
}

// ─── MCP Server State ─────────────────────────────────────────────────────────

pub struct McpServer {
    cpu_stats: CpuStats,
    mem_stats: MemStats,
    processes: Vec<ProcessInfo>,
    interfaces: Vec<NetInterface>,
    disks: Vec<DiskStat>,
    mounts: Vec<MountInfo>,
    sys_info: SysInfo,
    last_update: std::time::Instant,
    config: Config,
}

impl McpServer {
    pub fn new() -> Self {
        proc::init();

        let config = Config {
            interval_secs: 1,
            top_n: 10,
            color: false,
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
        };

        let mut server = Self {
            cpu_stats: CpuStats::default(),
            mem_stats: MemStats::default(),
            processes: Vec::new(),
            interfaces: Vec::new(),
            disks: Vec::new(),
            mounts: Vec::new(),
            sys_info: SysInfo::default(),
            last_update: std::time::Instant::now(),
            config,
        };

        // Initial CPU sample to seed delta counters
        cpu::update(&mut server.cpu_stats);
        std::thread::sleep(std::time::Duration::from_millis(100));
        server.refresh_all();

        server
    }

    fn refresh_all(&mut self) {
        let elapsed = self.last_update.elapsed().as_secs_f32().max(0.01);
        self.last_update = std::time::Instant::now();

        cpu::update(&mut self.cpu_stats);
        mem::update(&mut self.mem_stats);
        proc::update(
            &mut self.processes,
            self.mem_stats.total_kb,
            elapsed,
            self.config.top_n,
            None,
        );
        net::update(&mut self.interfaces, elapsed);
        disk::update_io(&mut self.disks, elapsed);
        disk::update_mounts(&mut self.mounts);
        sysinfo::update(&mut self.sys_info);
    }

    fn refresh_if_stale(&mut self) {
        if self.last_update.elapsed().as_secs() >= 1 {
            self.refresh_all();
        }
    }

    // ─── Tool Handlers ────────────────────────────────────────────────────────

    fn get_cpu_stats(&mut self) -> serde_json::Value {
        self.refresh_if_stale();

        let model = std::str::from_utf8(&self.cpu_stats.model[..self.cpu_stats.model_len])
            .unwrap_or("Unknown")
            .to_string();

        let cores: Vec<CoreInfo> = self
            .cpu_stats
            .cores
            .iter()
            .map(|c| CoreInfo {
                id: c.id,
                usage_percent: c.usage,
                freq_mhz: c.freq_mhz,
                temp_c: c.temp_c,
            })
            .collect();

        serde_json::to_value(CpuResponse {
            model,
            total_usage_percent: self.cpu_stats.total_usage,
            cores,
        })
        .unwrap()
    }

    fn get_memory_stats(&mut self) -> serde_json::Value {
        self.refresh_if_stale();

        let used_percent = if self.mem_stats.total_kb > 0 {
            (self.mem_stats.used_kb as f32 / self.mem_stats.total_kb as f32) * 100.0
        } else {
            0.0
        };

        serde_json::to_value(MemResponse {
            total_mb: self.mem_stats.total_kb / 1024,
            used_mb: self.mem_stats.used_kb / 1024,
            available_mb: self.mem_stats.available_kb / 1024,
            used_percent,
            swap_total_mb: self.mem_stats.swap_total_kb / 1024,
            swap_used_mb: self.mem_stats.swap_used_kb / 1024,
        })
        .unwrap()
    }

    fn get_network_stats(&mut self) -> serde_json::Value {
        self.refresh_if_stale();

        let interfaces: Vec<InterfaceInfo> = self
            .interfaces
            .iter()
            .map(|iface| {
                let name = iface.name_str().to_string();
                InterfaceInfo {
                    name,
                    rx_bytes_per_sec: iface.rx_rate as u64,
                    tx_bytes_per_sec: iface.tx_rate as u64,
                    rx_total_mb: iface.rx_bytes / (1024 * 1024),
                    tx_total_mb: iface.tx_bytes / (1024 * 1024),
                }
            })
            .collect();

        serde_json::to_value(NetResponse { interfaces }).unwrap()
    }

    fn get_disk_stats(&mut self) -> serde_json::Value {
        self.refresh_if_stale();

        let io_stats: Vec<DiskIoInfo> = self
            .disks
            .iter()
            .map(|d| {
                let name = d.name_str().to_string();
                DiskIoInfo {
                    name,
                    read_kb_per_sec: (d.read_rate / 1024.0) as u64,
                    write_kb_per_sec: (d.write_rate / 1024.0) as u64,
                }
            })
            .collect();

        let mounts: Vec<MountInfoResponse> = self
            .mounts
            .iter()
            .map(|m| {
                let device = m.device_str().to_string();
                let mount_point = m.mount_str().to_string();
                MountInfoResponse {
                    device,
                    mount_point,
                    total_gb: m.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                    used_gb: m.used_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                    available_gb: m.free_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                    used_percent: m.usage_percent,
                }
            })
            .collect();

        serde_json::to_value(DiskResponse { io_stats, mounts }).unwrap()
    }

    fn get_process_list(&mut self, top_n: Option<usize>) -> serde_json::Value {
        self.refresh_if_stale();

        let n = top_n.unwrap_or(self.config.top_n);
        let processes: Vec<ProcessInfoResponse> = self
            .processes
            .iter()
            .take(n)
            .map(|p| ProcessInfoResponse {
                pid: p.pid,
                name: p.name_str().to_string(),
                cpu_percent: p.cpu_usage,
                mem_mb: p.mem_kb / 1024,
                mem_percent: p.mem_percent,
                status: match p.status {
                    b'R' => "Running",
                    b'S' => "Sleeping",
                    b'D' => "Disk Sleep",
                    b'Z' => "Zombie",
                    b'T' => "Stopped",
                    b'I' => "Idle",
                    _ => "Unknown",
                }
                .to_string(),
            })
            .collect();

        serde_json::to_value(ProcessResponse { processes }).unwrap()
    }

    fn get_system_info(&mut self) -> serde_json::Value {
        self.refresh_if_stale();

        let hostname = self.sys_info.hostname_str().to_string();
        let kernel = self.sys_info.kernel_str().to_string();

        serde_json::to_value(SystemInfoResponse {
            hostname,
            kernel,
            uptime_secs: self.sys_info.uptime_secs,
            load_1: self.sys_info.load_1,
            load_5: self.sys_info.load_5,
            load_15: self.sys_info.load_15,
        })
        .unwrap()
    }

    // ─── MCP Protocol Handlers ────────────────────────────────────────────────

    fn handle_initialize(&self) -> serde_json::Value {
        let mut tools = HashMap::new();
        tools.insert("listChanged".to_string(), serde_json::json!(false));

        serde_json::to_value(McpInitializeResult {
            protocol_version: "2024-11-05",
            server_info: McpServerInfo {
                name: "zeroclaw",
                version: env!("CARGO_PKG_VERSION"),
            },
            capabilities: McpCapabilities { tools },
        })
        .unwrap()
    }

    fn handle_tools_list(&self) -> serde_json::Value {
        let tools = vec![
            McpTool {
                name: "get_cpu_stats",
                description: "Get CPU usage statistics including per-core usage, frequencies, and temperatures",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            McpTool {
                name: "get_memory_stats",
                description: "Get memory usage statistics including RAM and swap",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            McpTool {
                name: "get_network_stats",
                description: "Get network interface statistics including RX/TX rates",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            McpTool {
                name: "get_disk_stats",
                description: "Get disk I/O statistics and mount point usage",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
            McpTool {
                name: "get_process_list",
                description: "Get top processes sorted by CPU usage",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "top_n": {
                            "type": "integer",
                            "description": "Number of top processes to return (default: 10)"
                        }
                    },
                    "required": []
                }),
            },
            McpTool {
                name: "get_system_info",
                description: "Get system information including hostname, kernel version, uptime, and load averages",
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        ];

        serde_json::to_value(McpToolsListResult { tools }).unwrap()
    }

    fn handle_tool_call(
        &mut self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let result = match name {
            "get_cpu_stats" => self.get_cpu_stats(),
            "get_memory_stats" => self.get_memory_stats(),
            "get_network_stats" => self.get_network_stats(),
            "get_disk_stats" => self.get_disk_stats(),
            "get_process_list" => {
                let top_n = args
                    .get("top_n")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                self.get_process_list(top_n)
            }
            "get_system_info" => self.get_system_info(),
            _ => return Err(format!("Unknown tool: {}", name)),
        };

        Ok(serde_json::to_value(McpToolResult {
            content: vec![McpContent {
                content_type: "text",
                text: serde_json::to_string_pretty(&result).unwrap_or_default(),
            }],
            is_error: None,
        })
        .unwrap())
    }

    fn process_request(&mut self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let result = match request.method.as_str() {
            "initialize" => Ok(self.handle_initialize()),
            "initialized" => Ok(serde_json::json!({})),
            "tools/list" => Ok(self.handle_tools_list()),
            "tools/call" => {
                let name = request
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = request
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                self.handle_tool_call(name, &args)
            }
            "ping" => Ok(serde_json::json!({})),
            _ => Err(format!("Unknown method: {}", request.method)),
        };

        match result {
            Ok(res) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.clone(),
                result: Some(res),
                error: None,
            },
            Err(msg) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.clone(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: msg,
                }),
            },
        }
    }

    /// Run the MCP server, reading JSON-RPC requests from stdin and writing responses to stdout.
    pub fn run(&mut self) -> io::Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        eprintln!("[zeroclaw-mcp] Server started, waiting for requests...");

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => {
                    let response = self.process_request(&request);
                    let response_json = serde_json::to_string(&response)?;
                    writeln!(stdout, "{}", response_json)?;
                    stdout.flush()?;
                }
                Err(e) => {
                    let error_response = JsonRpcResponse {
                        jsonrpc: "2.0",
                        id: None,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                        }),
                    };
                    let response_json = serde_json::to_string(&error_response)?;
                    writeln!(stdout, "{}", response_json)?;
                    stdout.flush()?;
                }
            }
        }

        Ok(())
    }
}

/// Entry point for MCP server mode.
pub fn run_mcp_server() {
    let mut server = McpServer::new();
    if let Err(e) = server.run() {
        eprintln!("[zeroclaw-mcp] Error: {}", e);
        std::process::exit(1);
    }
}
