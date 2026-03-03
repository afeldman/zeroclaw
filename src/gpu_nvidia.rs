//! NVIDIA GPU monitoring via NVML (dynamically loaded).
//!
//! Uses dlopen to load libnvidia-ml.so at runtime, avoiding compile-time
//! dependencies. If the library is not found or fails to initialize,
//! GPU stats will be unavailable but the program continues normally.
//!
//! NVML provides:
//! - GPU utilization percentage
//! - Memory (VRAM) usage
//! - Temperature
//! - Power consumption
//! - Fan speed
//! - Clock frequencies

// Allow static mut refs for lazy NVML initialization (safe: single-threaded access)
#![allow(static_mut_refs)]

use crate::types::{GpuDevice, GpuStats};

/// NVML return types
const NVML_SUCCESS: u32 = 0;

/// NVML temperature sensor types
const NVML_TEMPERATURE_GPU: u32 = 0;

/// NVML clock types
const NVML_CLOCK_GRAPHICS: u32 = 0;
const NVML_CLOCK_MEM: u32 = 2;

/// Opaque handle to NVML device
type NvmlDevice = *mut libc::c_void;

/// NVML utilization struct
#[repr(C)]
struct NvmlUtilization {
    gpu: u32,
    memory: u32,
}

/// NVML memory info struct
#[repr(C)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

/// Function pointer types for NVML API
type NvmlInitFn = unsafe extern "C" fn() -> u32;
type NvmlShutdownFn = unsafe extern "C" fn() -> u32;
type NvmlDeviceGetCountFn = unsafe extern "C" fn(*mut u32) -> u32;
type NvmlDeviceGetHandleByIndexFn = unsafe extern "C" fn(u32, *mut NvmlDevice) -> u32;
type NvmlDeviceGetNameFn = unsafe extern "C" fn(NvmlDevice, *mut i8, u32) -> u32;
type NvmlDeviceGetUtilizationRatesFn =
    unsafe extern "C" fn(NvmlDevice, *mut NvmlUtilization) -> u32;
type NvmlDeviceGetMemoryInfoFn = unsafe extern "C" fn(NvmlDevice, *mut NvmlMemory) -> u32;
type NvmlDeviceGetTemperatureFn = unsafe extern "C" fn(NvmlDevice, u32, *mut u32) -> u32;
type NvmlDeviceGetPowerUsageFn = unsafe extern "C" fn(NvmlDevice, *mut u32) -> u32;
type NvmlDeviceGetFanSpeedFn = unsafe extern "C" fn(NvmlDevice, *mut u32) -> u32;
type NvmlDeviceGetClockInfoFn = unsafe extern "C" fn(NvmlDevice, u32, *mut u32) -> u32;

/// NVML library handle and function pointers
#[allow(dead_code)]
struct NvmlLib {
    handle: *mut libc::c_void,
    init: NvmlInitFn,
    shutdown: NvmlShutdownFn,
    device_get_count: NvmlDeviceGetCountFn,
    device_get_handle_by_index: NvmlDeviceGetHandleByIndexFn,
    device_get_name: NvmlDeviceGetNameFn,
    device_get_utilization_rates: NvmlDeviceGetUtilizationRatesFn,
    device_get_memory_info: NvmlDeviceGetMemoryInfoFn,
    device_get_temperature: NvmlDeviceGetTemperatureFn,
    device_get_power_usage: NvmlDeviceGetPowerUsageFn,
    device_get_fan_speed: NvmlDeviceGetFanSpeedFn,
    device_get_clock_info: NvmlDeviceGetClockInfoFn,
    initialized: bool,
}

static mut NVML: Option<NvmlLib> = None;

/// Load a symbol from the NVML library
unsafe fn load_sym<T>(handle: *mut libc::c_void, name: &[u8]) -> Option<T> {
    let sym = libc::dlsym(handle, name.as_ptr() as *const i8);
    if sym.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&sym))
    }
}

/// Initialize NVML library. Call once at startup.
pub fn init(stats: &mut GpuStats) {
    stats.available = false;
    stats.devices.clear();

    // Try to load libnvidia-ml.so
    let lib_names = [
        b"libnvidia-ml.so.1\0".as_ptr(),
        b"libnvidia-ml.so\0".as_ptr(),
    ];

    let mut handle: *mut libc::c_void = std::ptr::null_mut();
    for name in lib_names {
        handle = unsafe { libc::dlopen(name as *const i8, libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if !handle.is_null() {
            break;
        }
    }

    if handle.is_null() {
        set_error(stats, b"NVML library not found");
        return;
    }

    // Load function pointers
    unsafe {
        let init_fn: Option<NvmlInitFn> = load_sym(handle, b"nvmlInit_v2\0");
        let shutdown_fn: Option<NvmlShutdownFn> = load_sym(handle, b"nvmlShutdown\0");
        let count_fn: Option<NvmlDeviceGetCountFn> = load_sym(handle, b"nvmlDeviceGetCount_v2\0");
        let handle_fn: Option<NvmlDeviceGetHandleByIndexFn> =
            load_sym(handle, b"nvmlDeviceGetHandleByIndex_v2\0");
        let name_fn: Option<NvmlDeviceGetNameFn> = load_sym(handle, b"nvmlDeviceGetName\0");
        let util_fn: Option<NvmlDeviceGetUtilizationRatesFn> =
            load_sym(handle, b"nvmlDeviceGetUtilizationRates\0");
        let mem_fn: Option<NvmlDeviceGetMemoryInfoFn> =
            load_sym(handle, b"nvmlDeviceGetMemoryInfo\0");
        let temp_fn: Option<NvmlDeviceGetTemperatureFn> =
            load_sym(handle, b"nvmlDeviceGetTemperature\0");
        let power_fn: Option<NvmlDeviceGetPowerUsageFn> =
            load_sym(handle, b"nvmlDeviceGetPowerUsage\0");
        let fan_fn: Option<NvmlDeviceGetFanSpeedFn> = load_sym(handle, b"nvmlDeviceGetFanSpeed\0");
        let clock_fn: Option<NvmlDeviceGetClockInfoFn> =
            load_sym(handle, b"nvmlDeviceGetClockInfo\0");

        // Check all required functions are available
        let (
            init_fn,
            shutdown_fn,
            count_fn,
            handle_fn,
            name_fn,
            util_fn,
            mem_fn,
            temp_fn,
            power_fn,
            fan_fn,
            clock_fn,
        ) = match (
            init_fn,
            shutdown_fn,
            count_fn,
            handle_fn,
            name_fn,
            util_fn,
            mem_fn,
            temp_fn,
            power_fn,
            fan_fn,
            clock_fn,
        ) {
            (
                Some(a),
                Some(b),
                Some(c),
                Some(d),
                Some(e),
                Some(f),
                Some(g),
                Some(h),
                Some(i),
                Some(j),
                Some(k),
            ) => (a, b, c, d, e, f, g, h, i, j, k),
            _ => {
                libc::dlclose(handle);
                set_error(stats, b"NVML symbols not found");
                return;
            }
        };

        // Initialize NVML
        let ret = init_fn();
        if ret != NVML_SUCCESS {
            libc::dlclose(handle);
            set_error(stats, b"NVML init failed");
            return;
        }

        NVML = Some(NvmlLib {
            handle,
            init: init_fn,
            shutdown: shutdown_fn,
            device_get_count: count_fn,
            device_get_handle_by_index: handle_fn,
            device_get_name: name_fn,
            device_get_utilization_rates: util_fn,
            device_get_memory_info: mem_fn,
            device_get_temperature: temp_fn,
            device_get_power_usage: power_fn,
            device_get_fan_speed: fan_fn,
            device_get_clock_info: clock_fn,
            initialized: true,
        });
    }

    stats.available = true;
}

/// Update GPU statistics for all NVIDIA devices.
pub fn update(stats: &mut GpuStats) {
    let nvml = unsafe {
        match NVML.as_ref() {
            Some(n) if n.initialized => n,
            _ => {
                stats.available = false;
                return;
            }
        }
    };

    // Get device count
    let mut count: u32 = 0;
    let ret = unsafe { (nvml.device_get_count)(&mut count) };
    if ret != NVML_SUCCESS || count == 0 {
        stats.devices.clear();
        return;
    }

    // Ensure we have enough device entries
    while stats.devices.len() < count as usize {
        stats.devices.push(GpuDevice::default());
    }
    stats.devices.truncate(count as usize);

    // Query each device
    for i in 0..count {
        let device = &mut stats.devices[i as usize];
        device.index = i;

        let mut handle: NvmlDevice = std::ptr::null_mut();
        let ret = unsafe { (nvml.device_get_handle_by_index)(i, &mut handle) };
        if ret != NVML_SUCCESS {
            continue;
        }

        // Device name
        if device.name_len == 0 {
            let mut name_buf = [0i8; 64];
            let ret = unsafe { (nvml.device_get_name)(handle, name_buf.as_mut_ptr(), 64) };
            if ret == NVML_SUCCESS {
                let len = name_buf.iter().position(|&c| c == 0).unwrap_or(64);
                let copy_len = len.min(device.name.len());
                for j in 0..copy_len {
                    device.name[j] = name_buf[j] as u8;
                }
                device.name_len = copy_len;
            }
        }

        // Utilization rates
        let mut util = NvmlUtilization { gpu: 0, memory: 0 };
        let ret = unsafe { (nvml.device_get_utilization_rates)(handle, &mut util) };
        if ret == NVML_SUCCESS {
            device.utilization = util.gpu as f32;
            device.mem_utilization = util.memory as f32;
        }

        // Memory info
        let mut mem = NvmlMemory {
            total: 0,
            free: 0,
            used: 0,
        };
        let ret = unsafe { (nvml.device_get_memory_info)(handle, &mut mem) };
        if ret == NVML_SUCCESS {
            device.mem_total_mb = mem.total / (1024 * 1024);
            device.mem_used_mb = mem.used / (1024 * 1024);
        }

        // Temperature
        let mut temp: u32 = 0;
        let ret = unsafe { (nvml.device_get_temperature)(handle, NVML_TEMPERATURE_GPU, &mut temp) };
        device.temp_c = if ret == NVML_SUCCESS {
            Some(temp as f32)
        } else {
            None
        };

        // Power usage (milliwatts -> watts)
        let mut power: u32 = 0;
        let ret = unsafe { (nvml.device_get_power_usage)(handle, &mut power) };
        device.power_watts = if ret == NVML_SUCCESS {
            Some(power as f32 / 1000.0)
        } else {
            None
        };

        // Fan speed
        let mut fan: u32 = 0;
        let ret = unsafe { (nvml.device_get_fan_speed)(handle, &mut fan) };
        device.fan_percent = if ret == NVML_SUCCESS { Some(fan) } else { None };

        // GPU clock
        let mut clock: u32 = 0;
        let ret = unsafe { (nvml.device_get_clock_info)(handle, NVML_CLOCK_GRAPHICS, &mut clock) };
        device.clock_mhz = if ret == NVML_SUCCESS {
            Some(clock)
        } else {
            None
        };

        // Memory clock
        let mut mem_clock: u32 = 0;
        let ret = unsafe { (nvml.device_get_clock_info)(handle, NVML_CLOCK_MEM, &mut mem_clock) };
        device.mem_clock_mhz = if ret == NVML_SUCCESS {
            Some(mem_clock)
        } else {
            None
        };
    }
}

/// Shutdown NVML library. Call on program exit.
#[allow(dead_code)]
pub fn shutdown() {
    unsafe {
        if let Some(nvml) = NVML.take() {
            if nvml.initialized {
                (nvml.shutdown)();
            }
            libc::dlclose(nvml.handle);
        }
    }
}

/// Set an error message in GpuStats.
fn set_error(stats: &mut GpuStats, msg: &[u8]) {
    let mut err = [0u8; 128];
    let len = msg.len().min(err.len());
    err[..len].copy_from_slice(&msg[..len]);
    stats.error = Some(err);
    stats.error_len = len;
}
