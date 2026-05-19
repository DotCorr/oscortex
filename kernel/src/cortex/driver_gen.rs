//! Cortex Driver Generation — hot-loadable driver runtime.
//!
//! ## The Cortex Driver Protocol (CDP)
//!
//! All OSCortex drivers are WASM modules that expose a fixed ABI:
//!
//! ```wasm
//! ;; Required exports
//! (export "cdp_init"    (func $init))     ;; Called once on load
//! (export "cdp_probe"   (func $probe))    ;; Return 1 if driver handles this device
//! (export "cdp_open"    (func $open))     ;; Open the device
//! (export "cdp_close"   (func $close))    ;; Close the device
//! (export "cdp_read"    (func $read))     ;; Read data
//! (export "cdp_write"   (func $write))    ;; Write data
//! (export "cdp_ioctl"   (func $ioctl))    ;; Control operations
//! (export "cdp_irq"     (func $irq))      ;; IRQ handler (called from kernel shim)
//! (export "cdp_deinit"  (func $deinit))   ;; Called on unload
//! (export "cdp_version" (func $version))  ;; Return CDP ABI version
//! ```
//!
//! ## Why WASM?
//!
//! 1. **Portability** — one driver binary runs on x86_64, ARM64, RISC-V.
//! 2. **Safety** — WASM memory model provides Software Fault Isolation (SFI).
//!    A buggy driver cannot corrupt kernel memory outside its sandbox.
//! 3. **Hot-loading** — WASM modules are validated and JIT-compiled at load
//!    time. Swapping a driver is as simple as atomically replacing the function
//!    pointer table.
//! 4. **AI-generability** — The Cortex inference engine can emit WASM bytecode
//!    for unknown devices using hardware descriptor templates.
//!
//! ## Load flow
//!
//! 1. Device detected (PCI probe / USB enumeration / ACPI table walk).
//! 2. Driver registry lookup → no match.
//! 3. Cortex asked: "what drives PCI vendor:device 0xXXXX:0xYYYY?"
//! 4. Cortex inference engine consults context graph + model.
//! 5. Cortex returns WASM bytecode (from module store or AI-generated).
//! 6. `driver_gen::load()` validates + JIT-compiles the WASM.
//! 7. Driver is inserted into the live registry — no reboot.
//!
//! ## Healing flow
//!
//! 1. Driver panics / returns unrecoverable error.
//! 2. Cortex healing subsystem is notified.
//! 3. Driver is quarantined (calls routed to a null stub).
//! 4. Cortex generates/fetches a replacement WASM module.
//! 5. New driver is validated + loaded into the same registry slot.
//! 6. Quarantine lifted — system continues without userspace noticing.

use alloc::vec::Vec;

/// CDP ABI version this kernel understands.
pub const CDP_ABI_VERSION: u32 = 1;

/// Maximum number of simultaneously loaded drivers.
pub const MAX_DRIVERS: usize = 256;

/// A loaded driver instance.
pub struct DriverInstance {
    pub id:          u32,
    pub name:        [u8; 64],
    pub wasm_hash:   [u8; 32],    // SHA-256 of the WASM bytecode
    pub state:       DriverState,
    pub loaded_at:   u64,         // rdtsc timestamp
    pub error_count: u32,
    /// Vtable of resolved WASM function pointers (JIT-compiled).
    pub vtable:      DriverVtable,
    /// Live WASM sandbox — handles all CDP dispatch for WASM-backed drivers.
    pub sandbox: Option<alloc::boxed::Box<crate::drivers::wasm_sandbox::WasmSandbox>>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DriverState {
    Loading,
    Active,
    Quarantined,
    Unloading,
    Failed,
}

/// Function pointer table for CDP-compliant drivers.
pub struct DriverVtable {
    pub init:    Option<fn() -> i32>,
    pub probe:   Option<fn(device_id: u64) -> i32>,
    pub open:    Option<fn(minor: u32) -> i32>,
    pub close:   Option<fn(minor: u32) -> i32>,
    pub read:    Option<fn(buf: *mut u8, len: usize, off: u64) -> isize>,
    pub write:   Option<fn(buf: *const u8, len: usize, off: u64) -> isize>,
    pub ioctl:   Option<fn(cmd: u32, arg: u64) -> i64>,
    pub irq:     Option<fn(irq: u8) -> i32>,
    pub deinit:  Option<fn()>,
}

impl DriverVtable {
    pub const fn null() -> Self {
        Self {
            init: None, probe: None, open: None, close: None,
            read: None, write: None, ioctl: None, irq: None, deinit: None,
        }
    }
}

/// The live driver registry — a fixed-size array, no allocator needed.
pub struct DriverRegistry {
    drivers:   [Option<DriverInstance>; MAX_DRIVERS],
    count:     usize,
}

impl DriverRegistry {
    pub const fn new() -> Self {
        const NONE: Option<DriverInstance> = None;
        Self { drivers: [NONE; MAX_DRIVERS], count: 0 }
    }

    /// Load and register a driver from raw WASM bytes.
    ///
    /// Returns the driver id on success, or an error code.
    pub fn load(&mut self, name: &[u8], wasm: &[u8]) -> Result<u32, LoadError> {
        if self.count >= MAX_DRIVERS {
            return Err(LoadError::RegistryFull);
        }

        // 1. Validate WASM magic.
        if !wasm.starts_with(b"\x00asm") {
            return Err(LoadError::InvalidWasm);
        }

        // 2. Verify CDP ABI version export (simplified — real version uses
        //    a full WASM parser).
        // TODO: parse WASM export section and call cdp_version().

        // 3. Compute content hash (simplified — real version: SHA-256).
        let mut hash = [0u8; 32];
        for (i, b) in wasm.iter().enumerate() {
            hash[i % 32] ^= b;
        }

        // 4. JIT-compile (placeholder — real version: wasm3 / wasmtime no_std).
        // 4. Parse WASM into sandbox + validate cdp_version/cdp_init.
        let mut vtable = DriverVtable::null();
        let sandbox = load_wasm_driver(wasm, &mut vtable)?;

        // 5. Find a free slot.
        let id = self.count as u32;
        let mut driver_name = [0u8; 64];
        let copy_len = name.len().min(63);
        driver_name[..copy_len].copy_from_slice(&name[..copy_len]);

        self.drivers[self.count] = Some(DriverInstance {
            id,
            name: driver_name,
            wasm_hash: hash,
            state: DriverState::Active,  // sandbox already called cdp_init
            loaded_at: crate::arch::rdtsc(),
            error_count: 0,
            vtable,
            sandbox: Some(sandbox),
        });
        self.count += 1;

        let name_str = core::str::from_utf8(&driver_name[..copy_len]).unwrap_or("?");
        log::info!("[Cortex::DriverGen] Driver '{}' loaded via WASM sandbox (id={})", name_str, id);

        Ok(id)
    }

    /// Quarantine a driver — route all calls to null stubs.
    pub fn quarantine(&mut self, id: u32) {
        if let Some(Some(d)) = self.drivers.get_mut(id as usize) {
            d.state = DriverState::Quarantined;
            d.vtable = DriverVtable::null();
            log::warn!("[Cortex::DriverGen] Driver {} quarantined", id);
        }
    }

    /// Replace a quarantined driver with a new WASM module.
    pub fn replace(&mut self, id: u32, _name: &[u8], wasm: &[u8]) -> Result<(), LoadError> {
        if let Some(Some(d)) = self.drivers.get_mut(id as usize) {
            if d.state != DriverState::Quarantined {
                return Err(LoadError::NotQuarantined);
            }
            let mut vtable = DriverVtable::null();
            let sandbox = load_wasm_driver(wasm, &mut vtable)?;
            d.vtable     = vtable;
            d.sandbox    = Some(sandbox);
            d.error_count = 0;
            d.state      = DriverState::Active;
            log::info!("[Cortex::DriverGen] Driver {} hot-replaced via WASM sandbox (no reboot)", id);
            Ok(())
        } else {
            Err(LoadError::NotFound)
        }
    }
}

/// Minimal WASM binary for the built-in null driver — two exports:
///   cdp_version() -> i32  returns 1  (CDP ABI version)
///   cdp_init()    -> i32  returns 0  (success)
///
/// Hand-assembled from:
///   (module
///     (func (export "cdp_version") (result i32) i32.const 1)
///     (func (export "cdp_init")    (result i32) i32.const 0))
pub static NULL_DRIVER_WASM: &[u8] = &[
    // magic + version
    0x00, 0x61, 0x73, 0x6d,  0x01, 0x00, 0x00, 0x00,
    // type section: 1 type () -> (i32)
    0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f,
    // function section: 2 funcs both type 0
    0x03, 0x03, 0x02, 0x00, 0x00,
    // export section: "cdp_version"->func0, "cdp_init"->func1
    0x07, 0x1a, 0x02,
    0x0b, 0x63, 0x64, 0x70, 0x5f, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00,
    0x08, 0x63, 0x64, 0x70, 0x5f, 0x69, 0x6e, 0x69, 0x74, 0x00, 0x01,
    // code section: 2 bodies
    0x0a, 0x0b, 0x02,
    0x04, 0x00, 0x41, 0x01, 0x0b,   // cdp_version: i32.const 1
    0x04, 0x00, 0x41, 0x00, 0x0b,   // cdp_init:    i32.const 0
];

/// Parse and sandbox a WASM driver, then build a vtable that dispatches through it.
fn load_wasm_driver(
    wasm: &[u8],
    vtable: &mut DriverVtable,
) -> Result<alloc::boxed::Box<crate::drivers::wasm_sandbox::WasmSandbox>, LoadError> {
    use crate::drivers::wasm_sandbox::{WasmSandbox, WasmError};

    let sandbox = WasmSandbox::new(alloc::vec::Vec::from(wasm))
        .map_err(|e| {
            log::error!("[Cortex::DriverGen] WASM parse error: {:?}", e);
            LoadError::InvalidWasm
        })?;

    // Probe cdp_version — must return CDP_ABI_VERSION.
    match sandbox.call(b"cdp_version", &[]) {
        Ok(v) if v == CDP_ABI_VERSION as i32 => {}
        Ok(v) => {
            log::warn!("[Cortex::DriverGen] CDP version mismatch: got {} expected {}",
                v, CDP_ABI_VERSION);
            return Err(LoadError::VersionMismatch);
        }
        Err(WasmError::FunctionNotFound) => {} // optional in some drivers
        Err(e) => {
            log::error!("[Cortex::DriverGen] cdp_version exec error: {:?}", e);
            return Err(LoadError::InitFailed);
        }
    }

    // Run cdp_init — must return 0.
    match sandbox.call(b"cdp_init", &[]) {
        Ok(0) | Err(crate::drivers::wasm_sandbox::WasmError::FunctionNotFound) => {}
        Ok(rc) => {
            log::error!("[Cortex::DriverGen] cdp_init returned error code {}", rc);
            return Err(LoadError::InitFailed);
        }
        Err(e) => {
            log::error!("[Cortex::DriverGen] cdp_init exec error: {:?}", e);
            return Err(LoadError::InitFailed);
        }
    }

    // Null out native vtable — dispatch goes through sandbox.
    *vtable = DriverVtable::null();
    Ok(alloc::boxed::Box::new(sandbox))
}

/// JIT compilation stub — for drivers that bypass WASM (native C ABI, future).
fn jit_compile(_wasm: &[u8]) -> Result<DriverVtable, LoadError> {
    Ok(DriverVtable::null())
}

#[derive(Debug)]
pub enum LoadError {
    RegistryFull,
    InvalidWasm,
    VersionMismatch,
    InitFailed,
    NotFound,
    NotQuarantined,
}

// ── Subsystem interface ───────────────────────────────────────────────────────

static REGISTRY: spin::Mutex<DriverRegistry> = spin::Mutex::new(DriverRegistry::new());

pub fn init() {
    log::info!("[Cortex::DriverGen] CDP driver registry ready (capacity: {})", MAX_DRIVERS);

    // Load the built-in null driver to prove the WASM sandbox is working.
    let result = REGISTRY.lock().load(b"null-driver", NULL_DRIVER_WASM);
    match result {
        Ok(id) => log::info!("[Cortex::DriverGen] Built-in null driver loaded (id={}) — WASM sandbox OK", id),
        Err(e) => log::error!("[Cortex::DriverGen] Built-in driver load failed: {:?}", e),
    }
}

/// Ask the Cortex to find/generate a driver for the given device descriptor.
///
/// The device descriptor is an opaque byte slice — typically a serialised
/// PCI header, USB descriptor, or ACPI device path.
pub fn request_driver_for_device(device_desc: &[u8]) -> Option<u32> {
    // Build an inference query.
    let mut query = [0u8; 128];
    query[0] = 0x02; // tag: driver load request
    query[1] = 128;  // priority: high
    let copy = device_desc.len().min(126);
    query[2..2 + copy].copy_from_slice(&device_desc[..copy]);

    let deadline = crate::arch::rdtsc() + 10_000_000; // ~10 ms budget
    let result = crate::cortex::inference::run(&query[..2 + copy], deadline)?;

    if let crate::cortex::inference::InferenceResult::DriverRecommendation {
        module_name,
        content_hash: _,
        priority: _,
    } = result {
        // Look up the module in the embedded module store.
        // TODO: implement module store (embedded WASM blobs + signed update channel).
        let _ = module_name;
        log::info!("[Cortex::DriverGen] Driver requested for device — module store lookup TBD");
    }

    None
}
