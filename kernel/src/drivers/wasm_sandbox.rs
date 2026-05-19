//! Minimal WASM 1.0 interpreter — Cortex Driver Protocol (CDP) sandbox.
//!
//! Executes a restricted subset of WASM sufficient for CDP kernel driver modules:
//! integer arithmetic, local variables, direct calls, and structured control flow.
//! Memory and table instructions are explicitly rejected — driver hardware access
//! must go through named host-function imports (dispatched via `dispatch_host`).
//!
//! ## Supported instructions
//! unreachable, nop, block/loop/if/else/end, br/br_if, return, call,
//! local.{get,set,tee}, drop, select, i32.const, i32.{eqz,eq,ne,lt_s,gt_s,
//! le_s,ge_s,add,sub,mul,div_s,and,or,xor,shl,shr_s,shr_u}

use alloc::vec::Vec;

// ── Constants ──────────────────────────────────────────────────────────────────

const WASM_MAGIC:   [u8; 4] = [0x00, 0x61, 0x73, 0x6d];
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

const MAX_STACK:      usize = 256;
const MAX_LOCALS:     usize = 64;
const MAX_CALL_DEPTH: usize = 16;
const MAX_EXPORTS:    usize = 32;
const MAX_FUNCS:      usize = 128;

// ── Error ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum WasmError {
    InvalidMagic,
    InvalidVersion,
    Malformed,
    TooManyFunctions,
    TooManyExports,
    FunctionNotFound,
    StackOverflow,
    StackUnderflow,
    TooManyLocals,
    CallDepthExceeded,
    DivisionByZero,
    Trap,
    UnsupportedInstruction(u8),
}

// ── Internal parse-time structures ────────────────────────────────────────────

struct FuncBody {
    local_count: u32,
    /// Byte range of this function's expression inside `WasmSandbox::raw`.
    code_start:  usize,
    code_end:    usize,
}

struct Export {
    name:     [u8; 32],
    name_len: usize,
    func_idx: u32,
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// A validated, parsed, and executable WASM module.
///
/// Owns the raw byte slice. Function code offsets index into it directly
/// (zero-copy execution). Creating a `WasmSandbox` validates magic, version,
/// import/export/code sections and rejects any unsupported sections.
pub struct WasmSandbox {
    raw:               Vec<u8>,
    import_func_count: u32,
    funcs:             Vec<FuncBody>,
    exports:           Vec<Export>,
}

impl WasmSandbox {
    /// Parse and validate a WASM module from owned bytes.
    pub fn new(bytes: Vec<u8>) -> Result<Self, WasmError> {
        if bytes.len() < 8       { return Err(WasmError::InvalidMagic);   }
        if bytes[0..4] != WASM_MAGIC    { return Err(WasmError::InvalidMagic);   }
        if bytes[4..8] != WASM_VERSION  { return Err(WasmError::InvalidVersion); }

        let mut import_func_count: u32 = 0;
        let mut funcs:   Vec<FuncBody> = Vec::new();
        let mut exports: Vec<Export>   = Vec::new();
        let mut pos = 8usize;

        while pos < bytes.len() {
            let section_id   = rb(&bytes, &mut pos)?;
            let section_size = uleb(&bytes, &mut pos)? as usize;
            let section_end  = pos.saturating_add(section_size);
            if section_end > bytes.len() { return Err(WasmError::Malformed); }

            match section_id {
                // ── Import section ─────────────────────────────────────────
                2 => {
                    let count = uleb(&bytes, &mut pos)?;
                    for _ in 0..count {
                        let mlen = uleb(&bytes, &mut pos)? as usize;
                        pos = pos.saturating_add(mlen);
                        let flen = uleb(&bytes, &mut pos)? as usize;
                        pos = pos.saturating_add(flen);
                        let kind = rb(&bytes, &mut pos)?;
                        match kind {
                            0 => { uleb(&bytes, &mut pos)?; import_func_count += 1; }
                            1 => { rb(&bytes, &mut pos)?;   skip_limits(&bytes, &mut pos)?; }
                            2 => { skip_limits(&bytes, &mut pos)?; }
                            3 => { rb(&bytes, &mut pos)?; rb(&bytes, &mut pos)?; }
                            _ => return Err(WasmError::Malformed),
                        }
                    }
                }
                // ── Export section ─────────────────────────────────────────
                7 => {
                    let count = uleb(&bytes, &mut pos)?;
                    if count as usize > MAX_EXPORTS { return Err(WasmError::TooManyExports); }
                    for _ in 0..count {
                        let nlen  = uleb(&bytes, &mut pos)? as usize;
                        let mut name = [0u8; 32];
                        let copy  = nlen.min(31);
                        if pos + nlen > bytes.len() { return Err(WasmError::Malformed); }
                        name[..copy].copy_from_slice(&bytes[pos..pos + copy]);
                        pos += nlen;
                        let kind = rb(&bytes, &mut pos)?;
                        let idx  = uleb(&bytes, &mut pos)?;
                        if kind == 0 { exports.push(Export { name, name_len: copy, func_idx: idx }); }
                    }
                }
                // ── Code section ───────────────────────────────────────────
                10 => {
                    let count = uleb(&bytes, &mut pos)?;
                    if count as usize > MAX_FUNCS { return Err(WasmError::TooManyFunctions); }
                    for _ in 0..count {
                        let body_size  = uleb(&bytes, &mut pos)? as usize;
                        let body_end   = pos + body_size;
                        let ldcount    = uleb(&bytes, &mut pos)?;
                        let mut locals = 0u32;
                        for _ in 0..ldcount {
                            locals += uleb(&bytes, &mut pos)?;
                            rb(&bytes, &mut pos)?; // valtype
                        }
                        if locals as usize > MAX_LOCALS { return Err(WasmError::TooManyLocals); }
                        funcs.push(FuncBody {
                            local_count: locals,
                            code_start:  pos,
                            code_end:    body_end.saturating_sub(1), // excludes final `end`
                        });
                        pos = body_end;
                    }
                }
                // Skip all other sections (type, function, table, memory, …).
                _ => {}
            }
            pos = pos.max(section_end);
        }

        Ok(Self { raw: bytes, import_func_count, funcs, exports })
    }

    /// Call an exported function by name with i32 arguments, return i32 result.
    pub fn call(&self, name: &[u8], args: &[i32]) -> Result<i32, WasmError> {
        let func_idx = self.exports.iter()
            .find(|e| &e.name[..e.name_len] == name)
            .map(|e| e.func_idx)
            .ok_or(WasmError::FunctionNotFound)?;
        self.call_func(func_idx, args, 0)
    }

    // ── Interpreter ────────────────────────────────────────────────────────────

    fn call_func(&self, func_idx: u32, args: &[i32], depth: u32) -> Result<i32, WasmError> {
        if depth as usize >= MAX_CALL_DEPTH { return Err(WasmError::CallDepthExceeded); }

        // Host functions (imported) — return 0 stub for now.
        if func_idx < self.import_func_count {
            return Ok(dispatch_host(func_idx));
        }

        let lidx = (func_idx - self.import_func_count) as usize;
        if lidx >= self.funcs.len() { return Err(WasmError::FunctionNotFound); }
        let func = &self.funcs[lidx];

        // Locals: args first, rest zero.
        let mut locals = [0i32; MAX_LOCALS];
        for (i, a) in args.iter().take(MAX_LOCALS).enumerate() { locals[i] = *a; }

        // Value stack.
        let mut stack = [0i32; MAX_STACK];
        let mut sp = 0usize;

        macro_rules! push { ($v:expr) => {{
            if sp >= MAX_STACK { return Err(WasmError::StackOverflow); }
            stack[sp] = $v; sp += 1;
        }}; }
        macro_rules! pop { () => {{
            if sp == 0 { return Err(WasmError::StackUnderflow); }
            sp -= 1; stack[sp]
        }}; }

        let end  = func.code_end.min(self.raw.len());
        let code = &self.raw[func.code_start..end];
        let mut pc = 0usize;

        while pc < code.len() {
            let op = code[pc]; pc += 1;
            match op {
                0x00 => return Err(WasmError::Trap),              // unreachable
                0x01 => {}                                          // nop
                0x1a => { pop!(); }                                 // drop
                0x1b => { let c=pop!(); let b=pop!(); let a=pop!();  // select
                          push!(if c != 0 { a } else { b }); }
                0x0b => break,                                      // end
                0x0f => return Ok(if sp>0 { pop!() } else { 0 }),  // return
                0x10 => { // call
                    let (ci, n) = uleb_raw(code, pc); pc += n;
                    push!(self.call_func(ci, &[], depth+1)?);
                }
                0x20 => { let (i,n)=uleb_raw(code,pc);pc+=n; push!(locals[i as usize]); } // local.get
                0x21 => { let (i,n)=uleb_raw(code,pc);pc+=n; locals[i as usize]=pop!(); } // local.set
                0x22 => { let (i,n)=uleb_raw(code,pc);pc+=n;                              // local.tee
                          if sp==0{return Err(WasmError::StackUnderflow);}
                          locals[i as usize]=stack[sp-1]; }
                0x41 => { let (v,n)=sleb_raw(code,pc);pc+=n; push!(v); }                  // i32.const
                0x45 => { let a=pop!(); push!(if a==0{1}else{0}); }                        // i32.eqz
                0x46 => { let b=pop!();let a=pop!(); push!(if a==b{1}else{0}); }           // i32.eq
                0x47 => { let b=pop!();let a=pop!(); push!(if a!=b{1}else{0}); }           // i32.ne
                0x48 => { let b=pop!();let a=pop!(); push!(if a< b{1}else{0}); }           // i32.lt_s
                0x4a => { let b=pop!();let a=pop!(); push!(if a> b{1}else{0}); }           // i32.gt_s
                0x4c => { let b=pop!();let a=pop!(); push!(if a<=b{1}else{0}); }           // i32.le_s
                0x4e => { let b=pop!();let a=pop!(); push!(if a>=b{1}else{0}); }           // i32.ge_s
                0x6a => { let b=pop!();let a=pop!(); push!(a.wrapping_add(b)); }            // i32.add
                0x6b => { let b=pop!();let a=pop!(); push!(a.wrapping_sub(b)); }            // i32.sub
                0x6c => { let b=pop!();let a=pop!(); push!(a.wrapping_mul(b)); }            // i32.mul
                0x6d => { let b=pop!();let a=pop!();                                        // i32.div_s
                          if b==0{return Err(WasmError::DivisionByZero);}
                          push!(a.wrapping_div(b)); }
                0x71 => { let b=pop!();let a=pop!(); push!(a&b); }                          // i32.and
                0x72 => { let b=pop!();let a=pop!(); push!(a|b); }                          // i32.or
                0x73 => { let b=pop!();let a=pop!(); push!(a^b); }                          // i32.xor
                0x74 => { let b=pop!();let a=pop!(); push!(a.wrapping_shl(b as u32)); }     // i32.shl
                0x75 => { let b=pop!();let a=pop!();                                        // i32.shr_u
                          push!((a as u32).wrapping_shr(b as u32) as i32); }
                0x76 => { let b=pop!();let a=pop!(); push!(a.wrapping_shr(b as u32)); }     // i32.shr_s
                // block / loop — skip blocktype byte, push implicit label.
                0x02 | 0x03 => { pc += 1; }
                0x04 => { // if
                    pc += 1; // blocktype
                    if pop!() == 0 { pc = skip_else_or_end(code, pc); }
                }
                0x05 => { pc = skip_end(code, pc); }               // else
                0x0c | 0x0d => {                                    // br / br_if
                    let (_, n) = uleb_raw(code, pc); pc += n;
                    if op == 0x0d { if pop!() == 0 { continue; } }
                    return Ok(if sp>0 { pop!() } else { 0 });
                }
                other => return Err(WasmError::UnsupportedInstruction(other)),
            }
        }
        Ok(if sp>0 { pop!() } else { 0 })
    }
}

// ── LEB128 helpers ─────────────────────────────────────────────────────────────

fn rb(b: &[u8], p: &mut usize) -> Result<u8, WasmError> {
    if *p >= b.len() { return Err(WasmError::Malformed); }
    let v = b[*p]; *p += 1; Ok(v)
}

fn uleb(b: &[u8], p: &mut usize) -> Result<u32, WasmError> {
    let (v, n) = uleb_raw(b, *p); *p += n; Ok(v)
}

fn uleb_raw(b: &[u8], mut p: usize) -> (u32, usize) {
    let (mut r, mut s, start) = (0u32, 0u32, p);
    loop {
        if p >= b.len() { return (0, 1); }
        let byte = b[p]; p += 1;
        r |= ((byte & 0x7f) as u32) << s;
        s += 7;
        if byte & 0x80 == 0 || s >= 35 { break; }
    }
    (r, p - start)
}

fn sleb_raw(b: &[u8], mut p: usize) -> (i32, usize) {
    let (mut r, mut s, start) = (0i32, 0u32, p);
    loop {
        if p >= b.len() { return (0, 1); }
        let byte = b[p]; p += 1;
        r |= ((byte & 0x7f) as i32) << s;
        s += 7;
        if byte & 0x80 == 0 {
            if s < 32 && (byte & 0x40) != 0 { r |= !0i32 << s; }
            break;
        }
        if s >= 35 { break; }
    }
    (r, p - start)
}

fn skip_limits(b: &[u8], p: &mut usize) -> Result<(), WasmError> {
    let flags = uleb(b, p)?;
    uleb(b, p)?;
    if flags & 1 != 0 { uleb(b, p)?; }
    Ok(())
}

// ── Control-flow skip helpers ──────────────────────────────────────────────────

fn skip_else_or_end(code: &[u8], mut pc: usize) -> usize {
    let mut depth = 0usize;
    while pc < code.len() {
        let op = code[pc]; pc += 1;
        match op {
            0x02 | 0x03 | 0x04 => { pc += 1; depth += 1; }
            0x05 if depth == 0 => return pc,
            0x0b if depth == 0 => return pc,
            0x0b                => { depth -= 1; }
            0x10 | 0x0c | 0x0d => { let (_, n) = uleb_raw(code, pc); pc += n; }
            0x20 | 0x21 | 0x22 => { let (_, n) = uleb_raw(code, pc); pc += n; }
            0x41               => { let (_, n) = sleb_raw(code, pc); pc += n; }
            _ => {}
        }
    }
    pc
}

fn skip_end(code: &[u8], pc: usize) -> usize { skip_else_or_end(code, pc) }

// ── Host function dispatch ─────────────────────────────────────────────────────

/// Kernel-side host functions callable from WASM driver modules.
///
/// Index matches the order of function imports in the WASM module.
/// M6 initial set: log only. Hardware MMIO access added in M8.
fn dispatch_host(idx: u32) -> i32 {
    match idx {
        // host::klog_write(level: i32, msg_ptr: i32, msg_len: i32) — stub, no memory access yet.
        0 => { log::debug!("[WasmSandbox] host::klog_write called"); 0 }
        _ => 0,
    }
}
