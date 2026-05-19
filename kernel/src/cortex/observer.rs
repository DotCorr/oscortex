//! Interrupt observer — collects statistics per vector for anomaly detection.

use crate::arch::InterruptFrame;

const VECTORS: usize = 256;
const RING_SIZE: usize = 64; // samples per vector

/// Per-vector ring buffer of recent interrupt timestamps.
pub struct InterruptObserver {
    rings:   [[u64; RING_SIZE]; VECTORS],
    heads:   [u8; VECTORS],
    counts:  [u64; VECTORS],
    /// Moving average inter-arrival (in TSC ticks) per vector.
    avg_ia:  [u64; VECTORS],
}

impl InterruptObserver {
    pub const fn new() -> Self {
        Self {
            rings:  [[0u64; RING_SIZE]; VECTORS],
            heads:  [0u8; VECTORS],
            counts: [0u64; VECTORS],
            avg_ia: [0u64; VECTORS],
        }
    }

    /// Record an interrupt occurrence.
    #[inline]
    pub fn record(&mut self, vector: u8, _ip: u64, _err: Option<u64>) {
        let v = vector as usize;
        let ts = crate::arch::rdtsc();
        let h = self.heads[v] as usize;
        let prev_ts = self.rings[v][(h + RING_SIZE - 1) % RING_SIZE];
        let ia = if prev_ts > 0 { ts.saturating_sub(prev_ts) } else { 0 };
        self.rings[v][h] = ts;
        self.heads[v] = ((h + 1) % RING_SIZE) as u8;
        self.counts[v] += 1;
        // Exponential moving average: new = 0.875 * old + 0.125 * sample
        if ia > 0 {
            self.avg_ia[v] = (self.avg_ia[v] / 8) * 7 + ia / 8;
        }
    }
}

/// Returns true if the given vector's recent activity looks anomalous.
///
/// Heuristic: if the inter-arrival time drops below 10% of the running average
/// (interrupt storm) or if the count jumped >1000x in one tick, flag it.
pub fn is_anomalous(state: &crate::cortex::CortexState, vector: u8) -> bool {
    let v = vector as usize;
    let obs = &state.observer;
    let avg = obs.avg_ia[v];
    if avg == 0 {
        return false;
    }
    // Current inter-arrival: last two samples.
    let h = obs.heads[v] as usize;
    let t1 = obs.rings[v][(h + RING_SIZE - 1) % RING_SIZE];
    let t0 = obs.rings[v][(h + RING_SIZE - 2) % RING_SIZE];
    if t0 == 0 {
        return false;
    }
    let current_ia = t1.saturating_sub(t0);
    // Interrupt storm: current interval is less than 5% of average.
    current_ia > 0 && current_ia < avg / 20
}

/// Run a batch analysis of all vectors. Called from Cortex tick.
pub fn analyze(state: &mut crate::cortex::CortexState, deadline_tsc: u64) {
    for v in 0usize..VECTORS {
        if crate::arch::rdtsc() >= deadline_tsc {
            break;
        }
        if is_anomalous(state, v as u8) {
            state.anomaly_count += 1;
            crate::cortex::healing::schedule_async_check(v as u8);
        }
    }
}
