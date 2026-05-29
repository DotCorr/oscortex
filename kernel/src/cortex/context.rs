//! Cortex Context Graph — the kernel's growing memory of system state.
//!
//! The context graph records:
//!   - Devices (PCI, USB, platform) and their driver history
//!   - Process lineage (pid, ppid, binary hash, capability set)
//!   - I/O patterns per device (read/write rates, error counts)
//!   - Interrupt frequency histograms per vector
//!   - Driver load/unload events with timestamps
//!   - Anomaly events and their resolutions
//!
//! This gives the AI inference engine a queryable, temporally-aware picture
//! of the system so it can make informed driver-gen and healing decisions.
//!
//! Storage is a fixed-size arena in kernel memory (no heap realloc). When the
//! arena is full, the oldest/least-referenced nodes are evicted (LRU-like).

use spin::RwLock;

const MAX_NODES: usize = 4096;
const MAX_EDGES: usize = 16384;

/// A node in the context graph.
#[derive(Clone, Copy)]
pub struct ContextNode {
    pub kind:      NodeKind,
    pub id:        u64,
    pub timestamp: u64,   // rdtsc at insertion
    pub data:      [u8; 64], // serialised node payload (type-specific)
    pub refcount:  u32,
}

#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    Empty,
    Device,
    Process,
    Driver,
    Interrupt,
    AnomalyEvent,
    HealingAction,
}

/// A directed edge between two nodes.
#[derive(Clone, Copy)]
pub struct ContextEdge {
    pub from:      u32,
    pub to:        u32,
    pub kind:      EdgeKind,
    pub timestamp: u64,
}

#[derive(Clone, Copy)]
pub enum EdgeKind {
    Drives,       // device → driver
    Spawned,      // process → process
    Triggers,     // interrupt → handler
    Resolved,     // anomaly → healing action
    Depends,      // driver → driver
}

/// The full context graph — stored inline, no heap.
pub struct ContextGraph {
    nodes:      [ContextNode; MAX_NODES],
    edges:      [ContextEdge; MAX_EDGES],
    node_count: usize,
    edge_count: usize,
    pending:    [PendingEvent; 64],
    pending_wr: usize,
}

#[derive(Clone, Copy)]
struct PendingEvent {
    kind:   NodeKind,
    id:     u64,
    ts:     u64,
    data:   [u8; 64],
}

impl ContextGraph {
    pub const fn new() -> Self {
        Self {
            nodes:      [ContextNode {
                kind: NodeKind::Empty, id: 0,
                timestamp: 0, data: [0; 64], refcount: 0,
            }; MAX_NODES],
            edges:      [ContextEdge {
                from: 0, to: 0,
                kind: EdgeKind::Drives, timestamp: 0,
            }; MAX_EDGES],
            node_count: 0,
            edge_count: 0,
            pending:    [PendingEvent {
                kind: NodeKind::Empty, id: 0, ts: 0, data: [0; 64],
            }; 64],
            pending_wr: 0,
        }
    }

    /// Enqueue a node for insertion (non-blocking, called from interrupt paths).
    pub fn enqueue(&mut self, kind: NodeKind, id: u64, data: [u8; 64]) {
        let slot = self.pending_wr % 64;
        self.pending[slot] = PendingEvent {
            kind, id,
            ts: crate::arch::rdtsc(),
            data,
        };
        self.pending_wr = self.pending_wr.wrapping_add(1);
    }

    /// Drain pending events into the graph. Called from Cortex tick (non-IRQ).
    pub fn flush(&mut self) {
        let n = self.pending_wr.min(64);
        for i in 0..n {
            let ev = self.pending[i];
            if ev.kind != NodeKind::Empty {
                self.insert_node(ev.kind, ev.id, ev.ts, ev.data);
            }
        }
        self.pending_wr = 0;
    }

    fn insert_node(&mut self, kind: NodeKind, id: u64, ts: u64, data: [u8; 64]) {
        if self.node_count >= MAX_NODES {
            self.evict_oldest();
        }
        let slot = self.node_count;
        self.nodes[slot] = ContextNode { kind, id, timestamp: ts, data, refcount: 1 };
        self.node_count += 1;
    }

    fn evict_oldest(&mut self) {
        // Find node with smallest timestamp and zero refcount.
        let mut oldest_idx = 0;
        let mut oldest_ts = u64::MAX;
        for i in 0..self.node_count {
            if self.nodes[i].refcount == 0 && self.nodes[i].timestamp < oldest_ts {
                oldest_ts = self.nodes[i].timestamp;
                oldest_idx = i;
            }
        }
        // Slide remaining nodes down.
        self.nodes.copy_within((oldest_idx + 1)..self.node_count, oldest_idx);
        self.node_count -= 1;
    }

    /// Number of live nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.node_count
    }

    /// Query: count of nodes by kind.
    pub fn count_by_kind(&self, kind: NodeKind) -> usize {
        self.nodes[..self.node_count]
            .iter()
            .filter(|n| n.kind == kind)
            .count()
    }

    /// Find the most recent node matching `kind` and optional `id` (0 = any).
    pub fn find_node(&self, kind: NodeKind, id: u64) -> Option<&ContextNode> {
        self.nodes[..self.node_count]
            .iter()
            .rev()
            .find(|n| n.kind == kind && (id == 0 || n.id == id))
    }

    /// Serialise one node into `out`. Returns bytes written or 0 if no match.
    pub fn write_node(&self, kind: NodeKind, id: u64, out: &mut [u8]) -> usize {
        let Some(node) = self.find_node(kind, id) else {
            return 0;
        };
        if out.len() < 80 {
            return 0;
        }
        out[0] = node.kind as u8;
        out[1..9].copy_from_slice(&node.id.to_le_bytes());
        out[9..17].copy_from_slice(&node.timestamp.to_le_bytes());
        out[17..81].copy_from_slice(&node.data);
        80
    }

    /// Flat binary dump: header + up to `max_nodes` nodes (81 bytes each).
    pub fn dump_flat(&self, out: &mut [u8], max_nodes: usize) -> usize {
        const HDR: usize = 8;
        const NODE: usize = 81;
        if out.len() < HDR {
            return 0;
        }
        out[0..4].copy_from_slice(b"CTXG");
        out[4..6].copy_from_slice(&1u16.to_le_bytes());
        let n = self.node_count.min(max_nodes);
        out[6..8].copy_from_slice(&(n as u16).to_le_bytes());
        let mut off = HDR;
        for node in self.nodes[..n].iter() {
            if off + NODE > out.len() {
                break;
            }
            out[off] = node.kind as u8;
            out[off + 1..off + 9].copy_from_slice(&node.id.to_le_bytes());
            out[off + 9..off + 17].copy_from_slice(&node.timestamp.to_le_bytes());
            out[off + 17..off + 81].copy_from_slice(&node.data);
            off += NODE;
        }
        off
    }
}

// ── Subsystem interface ───────────────────────────────────────────────────────

static INIT_DONE: spin::Once<()> = spin::Once::new();

pub fn init() {
    INIT_DONE.call_once(|| {
        log::info!("[Cortex::Context] Graph initialised (capacity: {} nodes, {} edges)",
            MAX_NODES, MAX_EDGES);
    });
}

pub fn flush_pending(state: &mut super::CortexState) {
    state.context.flush();
}
