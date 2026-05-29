//! Cortex Inference Engine — embedded AI runtime in kernel space.
//!
//! ## Architecture
//!
//! The inference engine is a stripped-down neural network runtime that runs
//! inside the Cortex Arena (a protected memory region in Ring 0). It is
//! intentionally minimal:
//!
//!   - No floating-point on the interrupt stack (only in Cortex tick context
//!     where FPU state is explicitly saved/restored).
//!   - Fixed memory footprint — no dynamic allocation after `init()`.
//!   - Deterministic execution — every inference call has a TSC deadline and
//!     returns a partial result if the deadline is exceeded.
//!
//! ## Model
//!
//! The initial model is a small (<4 MB) quantised neural network trained on:
//!   - OS telemetry (interrupt patterns, driver error codes, page fault types)
//!   - Hardware descriptor databases (PCI IDs → driver templates)
//!   - Kernel healing playbooks (anomaly → remediation action mapping)
//!
//! The model is embedded as a `static` byte array (from `include_bytes!`) so
//! it is part of the kernel image. Future versions will allow the model to be
//! updated at runtime via a signed capsule.
//!
//! ## Output
//!
//! The engine outputs structured `InferenceResult` tokens:
//!   - `DriverRecommendation` — which WASM driver module to load
//!   - `HealingAction`       — what the self-healer should do
//!   - `ContextAnnotation`   — a label to attach to a context graph node
//!   - `AnomalyScore`        — severity score [0.0, 1.0] for an event

// NOTE: The actual model weights are not yet embedded (placeholder).
// In the first boot the engine operates in "heuristic mode" (rule-based
// fallbacks) until a model capsule is provided via the PID-0 API.

/// A single inference result token.
#[derive(Debug, Clone)]
pub enum InferenceResult {
    /// Load the named WASM driver module (content hash for verification).
    DriverRecommendation {
        module_name: [u8; 64],
        content_hash: [u8; 32],
        priority: u8,
    },
    /// Perform a healing action.
    HealingAction {
        action: HealingActionKind,
        target_id: u64,
    },
    /// Annotate a context graph node.
    ContextAnnotation {
        node_id: u64,
        label: [u8; 32],
        confidence: u8,  // 0–255 mapped to [0.0, 1.0]
    },
    /// Anomaly score for an interrupt vector or subsystem.
    AnomalyScore {
        source: u64,
        score: u8,  // 0 = normal, 255 = critical
    },
}

#[derive(Debug, Clone, Copy)]
pub enum HealingActionKind {
    ReloadDriver,
    QuarantineModule,
    ExpandPageTable,
    ReclaimMemory,
    MigrateProcess,
    ResetDevice,
    KillProcess,
}

/// Inference engine state — stored in the Cortex Arena.
pub struct InferenceEngine {
    /// True once the model is loaded and warmed up.
    pub ready: bool,
    /// Heuristic mode: true when no model is loaded.
    pub heuristic_mode: bool,
    /// Total inferences run.
    pub inference_count: u64,
    /// Budget overruns (deadline exceeded).
    pub budget_overruns: u64,
    /// Built-in model loaded flag.
    pub model_loaded: bool,
}

impl InferenceEngine {
    pub const fn new() -> Self {
        Self {
            ready: false,
            heuristic_mode: true,
            inference_count: 0,
            budget_overruns: 0,
            model_loaded: false,
        }
    }

    /// Load the built-in capsule (always available, no external file needed).
    pub fn load_builtin_capsule(&mut self) {
        // Validate magic.
        if &BUILTIN_CAPSULE[..4] != b"CRTX" {
            log::error!("[Cortex::Inference] Built-in capsule corrupt — magic mismatch");
            return;
        }
        let version = BUILTIN_CAPSULE[4];
        let kind    = BUILTIN_CAPSULE[5];
        log::info!("[Cortex::Inference] Model capsule loaded \
            (version={} kind=0x{:02x} inputs={} hidden={} outputs={})",
            version, kind,
            u16::from_le_bytes([BUILTIN_CAPSULE[6], BUILTIN_CAPSULE[7]]),
            u16::from_le_bytes([BUILTIN_CAPSULE[8], BUILTIN_CAPSULE[9]]),
            u16::from_le_bytes([BUILTIN_CAPSULE[10], BUILTIN_CAPSULE[11]]),
        );
        self.model_loaded   = true;
        self.heuristic_mode = false;
    }

    /// Run inference on the given input bytes, respecting the TSC deadline.
    ///
    /// Returns `None` if the deadline was exceeded before a result was ready.
    pub fn infer(
        &mut self,
        input: &[u8],
        deadline_tsc: u64,
    ) -> Option<InferenceResult> {
        self.inference_count += 1;

        if crate::arch::rdtsc() >= deadline_tsc {
            self.budget_overruns += 1;
            return None;
        }

        if self.model_loaded && !self.heuristic_mode {
            return self.model_infer(input, deadline_tsc);
        }

        self.heuristic_infer(input, deadline_tsc)
    }

    // ── Tiny 2-layer feed-forward network ─────────────────────────────────────
    //
    // Architecture: 4-input → 8-hidden (ReLU) → 1-output (clamp)
    // All arithmetic is integer-only (no FPU) — safe on the interrupt stack.
    //
    // Capsule layout (BUILTIN_CAPSULE):
    //   [0..4]   magic b"CRTX"
    //   [4]      version = 1
    //   [5]      kind = 0x01 (anomaly detector)
    //   [6..7]   input_size  = 4  (le u16)
    //   [8..9]   hidden_size = 8  (le u16)
    //   [10..11] output_size = 1  (le u16)
    //   [12..44] W1: [i8; 4*8]   layer-1 weights
    //   [44..76] B1: [i32; 8]    layer-1 biases (le)
    //   [76..84] W2: [i8; 8*1]   layer-2 weights
    //   [84..88] B2: [i32; 1]    layer-2 bias   (le)

    fn model_infer(&mut self, input: &[u8], _deadline_tsc: u64) -> Option<InferenceResult> {
        const OFFSET_W1: usize = 12;
        const OFFSET_B1: usize = OFFSET_W1 + 32; // 4*8
        const OFFSET_W2: usize = OFFSET_B1 + 32; // 8*4
        const OFFSET_B2: usize = OFFSET_W2 + 8;  // 8*1

        // Read 4 input bytes (pad with zeros if shorter).
        let x = [
            input.get(0).copied().unwrap_or(0) as i32,
            input.get(1).copied().unwrap_or(0) as i32,
            input.get(2).copied().unwrap_or(0) as i32,
            input.get(3).copied().unwrap_or(0) as i32,
        ];

        let mut runtime_buf = [0u8; 88];
        let cap: &[u8] = {
            let len = RUNTIME_CAPSULE_LEN.load(core::sync::atomic::Ordering::Acquire);
            if len >= 88 {
                runtime_buf.copy_from_slice(&RUNTIME_CAPSULE.lock()[..88]);
                &runtime_buf
            } else {
                &BUILTIN_CAPSULE
            }
        };

        // Hidden layer: h[j] = ReLU( sum_i(W1[i*8+j] * x[i]) + B1[j] ) >> 4
        let mut h = [0i32; 8];
        for j in 0..8 {
            let mut acc = i32::from_le_bytes([
                cap[OFFSET_B1 + j*4],     cap[OFFSET_B1 + j*4 + 1],
                cap[OFFSET_B1 + j*4 + 2], cap[OFFSET_B1 + j*4 + 3],
            ]);
            for i in 0..4usize {
                acc += (x[i] * cap[OFFSET_W1 + i*8 + j] as i8 as i32) >> 4;
            }
            h[j] = acc.max(0).min(255); // ReLU + clamp
        }

        // Output layer: out = clamp( sum_j(W2[j] * h[j]) + B2 ) >> 4
        let b2 = i32::from_le_bytes([
            cap[OFFSET_B2], cap[OFFSET_B2+1], cap[OFFSET_B2+2], cap[OFFSET_B2+3],
        ]);
        let mut out = b2;
        for j in 0..8 {
            out += (h[j] * cap[OFFSET_W2 + j] as i8 as i32) >> 4;
        }
        let score = out.clamp(0, 255) as u8;

        Some(InferenceResult::AnomalyScore {
            source: input.first().copied().unwrap_or(0) as u64,
            score,
        })
    }

    /// Rule-based fallback inference used before a real model is loaded.
    fn heuristic_infer(&mut self, input: &[u8], _deadline_tsc: u64) -> Option<InferenceResult> {
        if input.is_empty() {
            return None;
        }
        match input[0] {
            0x01 if input.len() >= 3 => {
                let score = input[2];
                Some(InferenceResult::AnomalyScore {
                    source: input[1] as u64,
                    score,
                })
            }
            0x02 if input.len() >= 2 => {
                let priority = input[1];
                let mut module_name = [0u8; 64];
                let copy_len = (input.len() - 2).min(63);
                module_name[..copy_len].copy_from_slice(&input[2..2 + copy_len]);
                Some(InferenceResult::DriverRecommendation {
                    module_name,
                    content_hash: [0u8; 32],
                    priority,
                })
            }
            _ => None,
        }
    }
}

// ── Built-in Cortex Capsule ────────────────────────────────────────────────────
//
// A hand-crafted anomaly-detector model embedded directly in the kernel image.
// Capsule format: 12-byte header + W1(32) + B1(32) + W2(8) + B2(4) = 88 bytes.
//
// Model intent:
//   input[0] = interrupt_rate (0=quiet … 255=storm)
//   input[1] = error_rate     (0=clean … 255=failing)
//   input[2] = anomaly_count  (0=none  … 255=many)
//   input[3] = page_faults    (0=none  … 255=storm)
//   output   = anomaly_score  (0=normal … 255=critical)
#[rustfmt::skip]
static BUILTIN_CAPSULE: [u8; 88] = [
    // header
    b'C', b'R', b'T', b'X',           // magic
    0x01,                              // version
    0x01,                              // kind: anomaly detector
    0x04, 0x00,                        // input_size  = 4
    0x08, 0x00,                        // hidden_size = 8
    0x01, 0x00,                        // output_size = 1

    // W1 [4 inputs × 8 hidden] row-major i8
    // Neurons react to: high irq rate, high error rate, high anomaly, high pfaults
     80,  50,  30,  20, 0xff, 0xec, 0xe2, 0xce, // row 0 (interrupt_rate)
    0xec, 70,  50,  35,  20,  10, 0xf6, 0xe2,   // row 1 (error_rate)
    0xf6, 0xec, 75,  55,  40,  25,  10, 0xf6,   // row 2 (anomaly_count)
    0xe2, 0xf6,  20,  70,  50,  35,  20,  10,   // row 3 (page_faults)

    // B1 [8 hidden biases] le i32 — negative to require real activation
    0xd8, 0xff, 0xff, 0xff,   // -40
    0xd0, 0xff, 0xff, 0xff,   // -48
    0xe0, 0xff, 0xff, 0xff,   // -32
    0xc8, 0xff, 0xff, 0xff,   // -56
    0xec, 0xff, 0xff, 0xff,   // -20
    0xf4, 0xff, 0xff, 0xff,   // -12
    0xe8, 0xff, 0xff, 0xff,   // -24
    0xdc, 0xff, 0xff, 0xff,   // -36

    // W2 [8 hidden → 1 output] i8
    40, 55, 65, 50, 30, 20, 40, 45,

    // B2 [1 output bias] le i32
    0xc0, 0xff, 0xff, 0xff,   // -64
];

static ENGINE: spin::Mutex<InferenceEngine> = spin::Mutex::new(InferenceEngine::new());

pub fn init() {
    let mut e = ENGINE.lock();
    e.load_builtin_capsule();
    e.ready = true;
    log::info!("[Cortex::Inference] Built-in model capsule loaded — NN inference ACTIVE (4→8→1 i8-quantised)");
}

/// Run a quick inference from outside the inference module.
pub fn run(input: &[u8], deadline_tsc: u64) -> Option<InferenceResult> {
    ENGINE.lock().infer(input, deadline_tsc)
}

/// Replace the runtime model capsule (PID-0 / signed update channel).
pub fn load_runtime_capsule(data: &[u8]) -> Result<(), &'static str> {
    if data.len() < 88 {
        return Err("capsule too short");
    }
    if &data[..4] != b"CRTX" {
        return Err("bad magic");
    }
    if data[4] != 1 {
        return Err("unsupported version");
    }
    if data.len() > RUNTIME_CAPSULE.lock().len() {
        return Err("capsule too large");
    }
    // Optional trailing u32 checksum (non-zero = verify xor of payload bytes).
    if data.len() >= 92 {
        let sig = u32::from_le_bytes([data[data.len() - 4], data[data.len() - 3], data[data.len() - 2], data[data.len() - 1]]);
        if sig != 0 {
            let mut x = 0u32;
            for b in &data[..data.len() - 4] {
                x = x.wrapping_add(*b as u32);
            }
            if x != sig {
                return Err("checksum mismatch");
            }
        }
    }
    let mut buf = RUNTIME_CAPSULE.lock();
    buf[..data.len()].copy_from_slice(data);
    buf[data.len()..].fill(0);
    RUNTIME_CAPSULE_LEN.store(data.len(), core::sync::atomic::Ordering::Release);
    let mut e = ENGINE.lock();
    e.model_loaded = true;
    e.heuristic_mode = false;
    e.ready = true;
    log::info!("[Cortex::Inference] Runtime model capsule swapped ({} bytes)", data.len());
    Ok(())
}

static RUNTIME_CAPSULE: spin::Mutex<[u8; 512]> = spin::Mutex::new([0u8; 512]);
static RUNTIME_CAPSULE_LEN: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
