//! Driver registry — manages native and CDP WASM drivers.

use crate::cortex::driver_gen::{DriverRegistry, LoadError};
use spin::Mutex;

static REGISTRY: Mutex<DriverRegistry> = Mutex::new(DriverRegistry::new());

pub fn init() {
    log::info!("[Drivers] Driver registry initialised");
}

/// Register a built-in kernel driver (no WASM sandbox).
pub fn register_native(name: &[u8]) -> Result<u32, LoadError> {
    REGISTRY.lock().register_native(name)
}

/// Load a WASM driver from bytes.
pub fn load(name: &[u8], wasm: &[u8]) -> Result<u32, LoadError> {
    REGISTRY.lock().load(name, wasm)
}

/// Quarantine a driver by id.
pub fn quarantine(id: u32) {
    REGISTRY.lock().quarantine(id);
}

/// Hot-replace a quarantined driver.
pub fn replace(id: u32, name: &[u8], wasm: &[u8]) -> Result<(), LoadError> {
    REGISTRY.lock().replace(id, name, wasm)
}

/// Number of registered drivers (native + WASM).
pub fn count() -> usize {
    REGISTRY.lock().count()
}

/// Access the live registry (Cortex driver_gen, healing).
pub(crate) fn with<F, R>(f: F) -> R
where
    F: FnOnce(&mut DriverRegistry) -> R,
{
    f(&mut *REGISTRY.lock())
}
