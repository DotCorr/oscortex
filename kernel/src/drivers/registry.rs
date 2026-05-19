//! Driver registry — manages all loaded kernel drivers.

use crate::cortex::driver_gen::{DriverRegistry, DriverState};
use spin::Mutex;

static REGISTRY: Mutex<DriverRegistry> = Mutex::new(DriverRegistry::new());

pub fn init() {
    log::info!("[Drivers] Driver registry initialised");
}

/// Load a WASM driver from bytes.
pub fn load(name: &[u8], wasm: &[u8]) -> Result<u32, crate::cortex::driver_gen::LoadError> {
    REGISTRY.lock().load(name, wasm)
}

/// Quarantine a driver by id.
pub fn quarantine(id: u32) {
    REGISTRY.lock().quarantine(id);
}

/// Hot-replace a quarantined driver.
pub fn replace(id: u32, name: &[u8], wasm: &[u8]) -> Result<(), crate::cortex::driver_gen::LoadError> {
    REGISTRY.lock().replace(id, name, wasm)
}
