//! Apple Metal/IOKit GPU monitoring for macOS.
//!
//! On Apple Silicon Macs, the GPU is integrated into the SoC and shares
//! unified memory with the CPU. This module uses IOKit to gather available
//! GPU statistics.
//!
//! Available metrics (depending on hardware):
//! - GPU name
//! - GPU utilization (via IOAccelerator)
//! - Some power metrics on supported devices
//!
//! Limitations:
//! - No separate VRAM on Apple Silicon (unified memory)
//! - Limited metrics compared to NVIDIA NVML
//! - Requires IOKit framework bindings

// Allow static mut refs for lazy IOKit initialization (safe: single-threaded access)
#![allow(static_mut_refs)]

use crate::types::{GpuDevice, GpuStats};

/// IOKit constants
const KERN_SUCCESS: i32 = 0;
const IO_OBJECT_NULL: u32 = 0;

// IOKit types
type MachPort = u32;
type IoIterator = u32;
type IoService = u32;
type IoRegistryEntry = u32;
type CfMutableDictionaryRef = *mut libc::c_void;
type CfStringRef = *const libc::c_void;
type CfTypeRef = *const libc::c_void;
type CfAllocatorRef = *const libc::c_void;

// CoreFoundation constants
const K_CF_ALLOCATOR_DEFAULT: CfAllocatorRef = std::ptr::null();
const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

/// Function pointer types for IOKit
type IOMasterPortFn = unsafe extern "C" fn(u32, *mut MachPort) -> i32;
type IOServiceMatchingFn = unsafe extern "C" fn(*const i8) -> CfMutableDictionaryRef;
type IOServiceGetMatchingServicesFn =
    unsafe extern "C" fn(MachPort, CfMutableDictionaryRef, *mut IoIterator) -> i32;
type IOIteratorNextFn = unsafe extern "C" fn(IoIterator) -> IoService;
type IORegistryEntryGetNameFn = unsafe extern "C" fn(IoRegistryEntry, *mut i8) -> i32;
type IORegistryEntryCreateCFPropertyFn =
    unsafe extern "C" fn(IoRegistryEntry, CfStringRef, CfAllocatorRef, u32) -> CfTypeRef;
type IOObjectReleaseFn = unsafe extern "C" fn(u32) -> i32;
type CfStringCreateWithCStringFn =
    unsafe extern "C" fn(CfAllocatorRef, *const i8, u32) -> CfStringRef;
type CfReleaseFn = unsafe extern "C" fn(CfTypeRef);
type CfGetTypeIdFn = unsafe extern "C" fn(CfTypeRef) -> u64;
type CfNumberGetTypeFn = unsafe extern "C" fn() -> u64;
type CfNumberGetValueFn = unsafe extern "C" fn(CfTypeRef, i32, *mut libc::c_void) -> bool;

/// IOKit library handles and function pointers
#[allow(dead_code)]
struct IOKitLib {
    iokit_handle: *mut libc::c_void,
    cf_handle: *mut libc::c_void,
    io_master_port: IOMasterPortFn,
    io_service_matching: IOServiceMatchingFn,
    io_service_get_matching_services: IOServiceGetMatchingServicesFn,
    io_iterator_next: IOIteratorNextFn,
    io_registry_entry_get_name: IORegistryEntryGetNameFn,
    io_registry_entry_create_cf_property: IORegistryEntryCreateCFPropertyFn,
    io_object_release: IOObjectReleaseFn,
    cf_string_create: CfStringCreateWithCStringFn,
    cf_release: CfReleaseFn,
    cf_get_type_id: CfGetTypeIdFn,
    cf_number_get_type_id: CfNumberGetTypeFn,
    cf_number_get_value: CfNumberGetValueFn,
    master_port: MachPort,
    initialized: bool,
}

static mut IOKIT: Option<IOKitLib> = None;

/// Load a symbol from a dylib
unsafe fn load_sym<T>(handle: *mut libc::c_void, name: &[u8]) -> Option<T> {
    let sym = libc::dlsym(handle, name.as_ptr() as *const i8);
    if sym.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&sym))
    }
}

/// Initialize IOKit for GPU monitoring.
pub fn init(stats: &mut GpuStats) {
    stats.available = false;
    stats.devices.clear();

    // Load IOKit framework
    let iokit_handle = unsafe {
        libc::dlopen(
            b"/System/Library/Frameworks/IOKit.framework/IOKit\0".as_ptr() as *const i8,
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        )
    };

    if iokit_handle.is_null() {
        set_error(stats, b"Failed to load IOKit");
        return;
    }

    // Load CoreFoundation
    let cf_handle = unsafe {
        libc::dlopen(
            b"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation\0".as_ptr()
                as *const i8,
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        )
    };

    if cf_handle.is_null() {
        unsafe {
            libc::dlclose(iokit_handle);
        }
        set_error(stats, b"Failed to load CoreFoundation");
        return;
    }

    // Load function pointers
    unsafe {
        let io_master_port: Option<IOMasterPortFn> = load_sym(iokit_handle, b"IOMasterPort\0");
        let io_service_matching: Option<IOServiceMatchingFn> =
            load_sym(iokit_handle, b"IOServiceMatching\0");
        let io_service_get_matching_services: Option<IOServiceGetMatchingServicesFn> =
            load_sym(iokit_handle, b"IOServiceGetMatchingServices\0");
        let io_iterator_next: Option<IOIteratorNextFn> =
            load_sym(iokit_handle, b"IOIteratorNext\0");
        let io_registry_entry_get_name: Option<IORegistryEntryGetNameFn> =
            load_sym(iokit_handle, b"IORegistryEntryGetName\0");
        let io_registry_entry_create_cf_property: Option<IORegistryEntryCreateCFPropertyFn> =
            load_sym(iokit_handle, b"IORegistryEntryCreateCFProperty\0");
        let io_object_release: Option<IOObjectReleaseFn> =
            load_sym(iokit_handle, b"IOObjectRelease\0");

        let cf_string_create: Option<CfStringCreateWithCStringFn> =
            load_sym(cf_handle, b"CFStringCreateWithCString\0");
        let cf_release: Option<CfReleaseFn> = load_sym(cf_handle, b"CFRelease\0");
        let cf_get_type_id: Option<CfGetTypeIdFn> = load_sym(cf_handle, b"CFGetTypeID\0");
        let cf_number_get_type_id: Option<CfNumberGetTypeFn> =
            load_sym(cf_handle, b"CFNumberGetTypeID\0");
        let cf_number_get_value: Option<CfNumberGetValueFn> =
            load_sym(cf_handle, b"CFNumberGetValue\0");

        // Check all required functions
        let (
            io_master_port,
            io_service_matching,
            io_service_get_matching_services,
            io_iterator_next,
            io_registry_entry_get_name,
            io_registry_entry_create_cf_property,
            io_object_release,
            cf_string_create,
            cf_release,
            cf_get_type_id,
            cf_number_get_type_id,
            cf_number_get_value,
        ) = match (
            io_master_port,
            io_service_matching,
            io_service_get_matching_services,
            io_iterator_next,
            io_registry_entry_get_name,
            io_registry_entry_create_cf_property,
            io_object_release,
            cf_string_create,
            cf_release,
            cf_get_type_id,
            cf_number_get_type_id,
            cf_number_get_value,
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
                Some(l),
            ) => (a, b, c, d, e, f, g, h, i, j, k, l),
            _ => {
                libc::dlclose(iokit_handle);
                libc::dlclose(cf_handle);
                set_error(stats, b"Failed to load IOKit symbols");
                return;
            }
        };

        // Get master port
        let mut master_port: MachPort = 0;
        let ret = io_master_port(0, &mut master_port);
        if ret != KERN_SUCCESS {
            libc::dlclose(iokit_handle);
            libc::dlclose(cf_handle);
            set_error(stats, b"Failed to get IOKit master port");
            return;
        }

        IOKIT = Some(IOKitLib {
            iokit_handle,
            cf_handle,
            io_master_port,
            io_service_matching,
            io_service_get_matching_services,
            io_iterator_next,
            io_registry_entry_get_name,
            io_registry_entry_create_cf_property,
            io_object_release,
            cf_string_create,
            cf_release,
            cf_get_type_id,
            cf_number_get_type_id,
            cf_number_get_value,
            master_port,
            initialized: true,
        });
    }

    stats.available = true;

    // Initial GPU discovery
    discover_gpus(stats);
}

/// Discover available GPUs via IOKit
fn discover_gpus(stats: &mut GpuStats) {
    let iokit = unsafe {
        match IOKIT.as_ref() {
            Some(k) if k.initialized => k,
            _ => return,
        }
    };

    stats.devices.clear();

    // Look for IOAccelerator (GPU) devices
    let matching = unsafe { (iokit.io_service_matching)(b"IOAccelerator\0".as_ptr() as *const i8) };
    if matching.is_null() {
        return;
    }

    let mut iterator: IoIterator = IO_OBJECT_NULL;
    let ret = unsafe {
        (iokit.io_service_get_matching_services)(iokit.master_port, matching, &mut iterator)
    };

    if ret != KERN_SUCCESS || iterator == IO_OBJECT_NULL {
        return;
    }

    let mut gpu_index = 0u32;
    loop {
        let service = unsafe { (iokit.io_iterator_next)(iterator) };
        if service == IO_OBJECT_NULL {
            break;
        }

        let mut device = GpuDevice::default();
        device.index = gpu_index;

        // Get device name
        let mut name_buf = [0i8; 128];
        let ret = unsafe { (iokit.io_registry_entry_get_name)(service, name_buf.as_mut_ptr()) };
        if ret == KERN_SUCCESS {
            let len = name_buf.iter().position(|&c| c == 0).unwrap_or(128);
            let copy_len = len.min(device.name.len());
            for i in 0..copy_len {
                device.name[i] = name_buf[i] as u8;
            }
            device.name_len = copy_len;
        }

        // If name is generic, try to get the model
        if device.name_len == 0 || device.name.starts_with(b"IOAccelerator") {
            // Try to get a better name from sysctl
            get_gpu_name_from_sysctl(&mut device);
        }

        stats.devices.push(device);
        gpu_index += 1;

        unsafe {
            (iokit.io_object_release)(service);
        }
    }

    unsafe {
        (iokit.io_object_release)(iterator);
    }
}

/// Try to get GPU name from sysctl on Apple Silicon
fn get_gpu_name_from_sysctl(device: &mut GpuDevice) {
    // On Apple Silicon, the GPU is part of the chip, so use chip name
    let mut buf = [0u8; 64];
    let mut len = buf.len();

    // Try machdep.cpu.brand_string first (includes chip info)
    let name = b"machdep.cpu.brand_string\0";
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr() as *const i8,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };

    if ret == 0 && len > 0 {
        // Extract just the chip model (e.g., "Apple M4 Max")
        let copy_len = (len - 1).min(device.name.len());
        device.name[..copy_len].copy_from_slice(&buf[..copy_len]);
        device.name_len = copy_len;

        // Append " GPU" suffix
        let suffix = b" GPU";
        if device.name_len + suffix.len() < device.name.len() {
            device.name[device.name_len..device.name_len + suffix.len()].copy_from_slice(suffix);
            device.name_len += suffix.len();
        }
    }
}

/// Update GPU statistics. Note: Limited metrics available on macOS.
pub fn update(stats: &mut GpuStats) {
    let iokit = unsafe {
        match IOKIT.as_ref() {
            Some(k) if k.initialized => k,
            _ => {
                stats.available = false;
                return;
            }
        }
    };

    // Re-discover GPUs if list is empty
    if stats.devices.is_empty() {
        discover_gpus(stats);
    }

    // Query IOAccelerator statistics
    let matching = unsafe { (iokit.io_service_matching)(b"IOAccelerator\0".as_ptr() as *const i8) };
    if matching.is_null() {
        return;
    }

    let mut iterator: IoIterator = IO_OBJECT_NULL;
    let ret = unsafe {
        (iokit.io_service_get_matching_services)(iokit.master_port, matching, &mut iterator)
    };

    if ret != KERN_SUCCESS || iterator == IO_OBJECT_NULL {
        return;
    }

    let mut gpu_index = 0usize;
    loop {
        let service = unsafe { (iokit.io_iterator_next)(iterator) };
        if service == IO_OBJECT_NULL {
            break;
        }

        if gpu_index < stats.devices.len() {
            let device = &mut stats.devices[gpu_index];

            // Try to get GPU utilization from PerformanceStatistics
            if let Some(util) = get_gpu_utilization(iokit, service) {
                device.utilization = util;
            }

            // Memory on Apple Silicon uses unified memory, show system memory usage
            // This is less useful but provides some indication
            device.mem_total_mb = 0;
            device.mem_used_mb = 0;
            device.mem_utilization = 0.0;

            // Temperature might be available via SMC but requires additional APIs
            device.temp_c = None;
            device.power_watts = None;
            device.fan_percent = None;
            device.clock_mhz = None;
            device.mem_clock_mhz = None;
        }

        gpu_index += 1;
        unsafe {
            (iokit.io_object_release)(service);
        }
    }

    unsafe {
        (iokit.io_object_release)(iterator);
    }
}

/// Try to get GPU utilization from IOAccelerator statistics
fn get_gpu_utilization(iokit: &IOKitLib, service: IoService) -> Option<f32> {
    // Create CFString for property name
    let prop_name = unsafe {
        (iokit.cf_string_create)(
            K_CF_ALLOCATOR_DEFAULT,
            b"PerformanceStatistics\0".as_ptr() as *const i8,
            K_CF_STRING_ENCODING_UTF8,
        )
    };

    if prop_name.is_null() {
        return None;
    }

    // Get the property dictionary
    let props = unsafe {
        (iokit.io_registry_entry_create_cf_property)(service, prop_name, K_CF_ALLOCATOR_DEFAULT, 0)
    };

    unsafe {
        (iokit.cf_release)(prop_name as CfTypeRef);
    }

    if props.is_null() {
        return None;
    }

    // The PerformanceStatistics dictionary contains GPU utilization info
    // but extracting it requires more CoreFoundation dictionary APIs
    // For now, we return a placeholder
    unsafe {
        (iokit.cf_release)(props);
    }

    // Note: Full implementation would need CFDictionaryGetValue to extract
    // "Device Utilization %" or similar keys from PerformanceStatistics
    None
}

/// Shutdown IOKit handles
#[allow(dead_code)]
pub fn shutdown() {
    unsafe {
        if let Some(iokit) = IOKIT.take() {
            if iokit.initialized {
                libc::dlclose(iokit.iokit_handle);
                libc::dlclose(iokit.cf_handle);
            }
        }
    }
}

/// Set an error message in GpuStats
fn set_error(stats: &mut GpuStats, msg: &[u8]) {
    let mut err = [0u8; 128];
    let len = msg.len().min(err.len());
    err[..len].copy_from_slice(&msg[..len]);
    stats.error = Some(err);
    stats.error_len = len;
}
