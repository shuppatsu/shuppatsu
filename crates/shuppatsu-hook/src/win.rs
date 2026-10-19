//! The proxy DLL itself (Windows only). Loaded as `dinput.dll` from the game's BIN directory via
//! `WINEDLLOVERRIDES=dinput=n,b`; forwards `DirectInputCreateA` to the system DirectInput.
//!
//! On load it redirects the exe's import of `user32!PeekMessageA`. The game calls it once per
//! tick from its message pump (0x42a130, inside Task_Tick 0x4253c0, after the global tick
//! counter 0x84c494 has advanced), so the redirect sees every tick. No game code is modified.
//!
//! Configuration (environment, passed through by Wine; paths in Windows form, e.g. `Z:\...`):
//! - `SHUPPATSU_HOOK_SCHEMA`: trace schema (see `schema.rs`); required for tracing.
//! - `SHUPPATSU_HOOK_TRACE`: CSV output path.
//! - `SHUPPATSU_HOOK_INPUTS`: optional input script (see `script.rs`).
//! - `SHUPPATSU_HOOK_LOG`: optional diagnostics log.

use crate::schema::{self, Address, Field};
use crate::script::Script;
use std::ffi::c_void;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

type Hmodule = *mut c_void;
type PeekMessageFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32, u32, u32) -> i32;
type DirectInputCreateFn =
    unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut c_void) -> i32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> Hmodule;
    fn LoadLibraryA(name: *const u8) -> Hmodule;
    fn GetProcAddress(module: Hmodule, name: *const u8) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, prot: u32, old: *mut u32) -> i32;
}

const PAGE_READWRITE: u32 = 0x04;
const DLL_PROCESS_ATTACH: u32 = 1;
const E_FAIL: i32 = 0x8000_4005_u32 as i32;

/// Globals of the original (docs/re/main-loop.md, input.md).
const GLOBAL_TICK: u32 = 0x84c494;
const PHASE_FLAGS: u32 = 0x855ff8;
/// Phase flag bit set while the driving loop runs (main-loop.md §5, likely).
const PHASE_DRIVING: u32 = 0x200;
const POWER_NOTCH: u32 = 0x829d00;
/// Task list 2 (head, tail, disabled; main-loop.md §2) and the physics task's update function.
const TASK_LIST_2: u32 = 0x84c198;
const PHYSICS_UPDATE: u32 = 0x422c20;
const BRAKE_NOTCH: u32 = 0x829d04;

static ORIGINAL_PEEK: AtomicUsize = AtomicUsize::new(0);
static STATE: Mutex<Option<HookState>> = Mutex::new(None);

struct HookState {
    image_base: usize,
    image_size: usize,
    fields: Vec<Field>,
    trace: Option<BufWriter<File>>,
    script: Option<Script>,
    log: Option<File>,
    last_tick: Option<u32>,
    driving_tick: Option<u32>,
    rows: u64,
}

impl HookState {
    fn log(&mut self, msg: &str) {
        if let Some(f) = &mut self.log {
            let _ = writeln!(f, "{msg}");
        }
    }

    /// Reads `len` bytes at a VA inside the exe image.
    fn read(&self, va: u32, len: usize) -> Option<Vec<u8>> {
        let start = (va as usize).checked_sub(self.image_base)?;
        if start + len > self.image_size {
            return None;
        }
        let mut out = vec![0u8; len];
        // SAFETY: `va..va+len` lies inside the exe's mapped image (checked against SizeOfImage
        // above); the loader maps the whole image readable, and these are plain data globals.
        unsafe { std::ptr::copy_nonoverlapping(va as usize as *const u8, out.as_mut_ptr(), len) };
        Some(out)
    }

    /// Reads `len` bytes of a heap object (a task allocated by the game). Only used for
    /// pointers found by walking the game's own task list.
    fn read_heap(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        if addr < 0x10000 {
            return None;
        }
        let mut out = vec![0u8; len];
        // SAFETY: `addr` comes from the game's live task list (see `find_physics_task`), i.e. a
        // malloc'd task object of at least 0x30 + task data bytes; we read on the game thread,
        // so the list cannot change concurrently. Offsets come from docs/re/physics.md §8.
        unsafe { std::ptr::copy_nonoverlapping(addr as usize as *const u8, out.as_mut_ptr(), len) };
        Some(out)
    }

    /// Walks task list 2 for the task whose update function is the physics task's.
    fn find_physics_task(&self) -> Option<u32> {
        let mut node = self.read_u32(TASK_LIST_2)?;
        for _ in 0..1024 {
            if node == 0 {
                return None;
            }
            let update = self.read_heap(node + 8, 4)?;
            if u32::from_le_bytes([update[0], update[1], update[2], update[3]]) == PHYSICS_UPDATE {
                return Some(node);
            }
            let next = self.read_heap(node + 4, 4)?;
            node = u32::from_le_bytes([next[0], next[1], next[2], next[3]]);
        }
        None
    }

    fn read_u32(&self, va: u32) -> Option<u32> {
        self.read(va, 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn write_i32(&self, va: u32, value: i32) -> bool {
        let Some(start) = (va as usize).checked_sub(self.image_base) else {
            return false;
        };
        if start + 4 > self.image_size {
            return false;
        }
        // SAFETY: inside the exe image (checked above) in its writable data/BSS section; the
        // game thread is the caller (we run inside its message pump), so there is no
        // concurrent access to these input globals.
        unsafe { (va as usize as *mut i32).write_unaligned(value) };
        true
    }

    /// Per-tick work, called from the redirected PeekMessageA.
    fn on_pump(&mut self) {
        let Some(tick) = self.read_u32(GLOBAL_TICK) else {
            return;
        };
        if self.last_tick == Some(tick) {
            return;
        }
        self.last_tick = Some(tick);
        let driving = self
            .read_u32(PHASE_FLAGS)
            .is_some_and(|f| f & PHASE_DRIVING != 0);
        self.driving_tick = if driving {
            Some(self.driving_tick.map_or(0, |t| t + 1))
        } else {
            None
        };
        if let (Some(dt), Some(script)) = (self.driving_tick, &self.script)
            && let Some((notch, brake)) = script.at(dt)
        {
            self.write_i32(POWER_NOTCH, notch);
            self.write_i32(BRAKE_NOTCH, brake);
        }
        if self.trace.is_some() {
            let physics_task = self.find_physics_task();
            let mut row = String::new();
            row.push_str(&self.driving_tick.map_or(-1, i64::from).to_string());
            for f in &self.fields {
                row.push(',');
                let value = if f.ty == schema::FieldType::FpuCw {
                    Some(fpu_control_word().to_le_bytes().to_vec())
                } else {
                    match f.address {
                        Address::Global(va) => self.read(va, f.ty.size()),
                        Address::PhysicsTask(off) => physics_task
                            .and_then(|t| self.read_heap(t.wrapping_add(off), f.ty.size())),
                    }
                };
                match value {
                    Some(b) => f.ty.format(&b, &mut row),
                    None => row.push('?'),
                }
            }
            if let Some(t) = &mut self.trace {
                let _ = writeln!(t, "{row}");
                self.rows += 1;
                if self.rows.is_multiple_of(30) {
                    let _ = t.flush();
                }
            }
        }
    }
}

/// The x87 control word of the current thread (precision bits 8–9: 00 = 24-bit, 10 = 53-bit,
/// 11 = 64-bit), to settle the physics precision question (physics.md).
fn fpu_control_word() -> u16 {
    let mut cw: u16 = 0;
    // SAFETY: `fnstcw` only stores the control word to the given memory; no other state changes.
    unsafe { std::arch::asm!("fnstcw [{}]", in(reg) &mut cw, options(nostack)) };
    cw
}

unsafe extern "system" fn peek_message_hook(
    msg: *mut c_void,
    hwnd: *mut c_void,
    min: u32,
    max: u32,
    remove: u32,
) -> i32 {
    if let Ok(mut guard) = STATE.lock()
        && let Some(state) = guard.as_mut()
    {
        state.on_pump();
    }
    let original = ORIGINAL_PEEK.load(Ordering::Acquire);
    if original == 0 {
        return 0;
    }
    // SAFETY: `original` is the address of user32!PeekMessageA saved from the IAT before it was
    // replaced; the signature matches the Win32 declaration.
    unsafe {
        let f: PeekMessageFn = std::mem::transmute::<usize, PeekMessageFn>(original);
        f(msg, hwnd, min, max, remove)
    }
}

/// Reads a u32 at `base + off` of the loaded exe image.
///
/// # Safety
/// `base` must be the exe's image base and `off` within its headers or mapped image.
unsafe fn rd32(base: usize, off: usize) -> u32 {
    // SAFETY: guaranteed by the caller.
    unsafe { ((base + off) as *const u32).read_unaligned() }
}

/// Case-insensitive comparison of a NUL-terminated string in the image with `name`.
///
/// # Safety
/// `ptr` must point to a NUL-terminated string.
unsafe fn cstr_eq(ptr: *const u8, name: &str) -> bool {
    // SAFETY: guaranteed by the caller.
    let s = unsafe { std::ffi::CStr::from_ptr(ptr.cast()) };
    s.to_bytes().eq_ignore_ascii_case(name.as_bytes())
}

/// Replaces the exe's IAT entry for `dll!func` with `hook`; returns the original pointer.
///
/// # Safety
/// `base` must be the loaded exe's image base.
unsafe fn patch_iat(base: usize, dll: &str, func: &str, hook: usize) -> Option<usize> {
    // SAFETY: walking the loader-validated PE headers and import directory of our own exe.
    unsafe {
        let pe = rd32(base, 0x3c) as usize;
        let opt = pe + 24;
        let import_rva = rd32(base, opt + 96 + 8) as usize;
        if import_rva == 0 {
            return None;
        }
        let mut desc = base + import_rva;
        loop {
            let ilt = rd32(desc, 0) as usize;
            let name = rd32(desc, 12) as usize;
            let iat = rd32(desc, 16) as usize;
            if name == 0 {
                return None;
            }
            if cstr_eq((base + name) as *const u8, dll) {
                let names = if ilt != 0 { ilt } else { iat };
                let mut i = 0;
                loop {
                    let entry = rd32(base, names + i * 4) as usize;
                    if entry == 0 {
                        break;
                    }
                    if entry & 0x8000_0000 == 0 && cstr_eq((base + entry + 2) as *const u8, func) {
                        let slot = (base + iat + i * 4) as *mut usize;
                        let mut old = 0u32;
                        if VirtualProtect(slot.cast(), 4, PAGE_READWRITE, &mut old) == 0 {
                            return None;
                        }
                        let original = slot.read_unaligned();
                        slot.write_unaligned(hook);
                        VirtualProtect(slot.cast(), 4, old, &mut old);
                        return Some(original);
                    }
                    i += 1;
                }
            }
            desc += 20;
        }
    }
}

fn install() {
    let env_file = |var: &str| std::env::var(var).ok().filter(|s| !s.is_empty());
    let log = env_file("SHUPPATSU_HOOK_LOG").and_then(|p| File::create(p).ok());
    // SAFETY: GetModuleHandleA(NULL) returns the exe's base, valid for the process lifetime.
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    // SAFETY: base is the exe image; SizeOfImage is at optional header + 56.
    let image_size = unsafe { rd32(base, rd32(base, 0x3c) as usize + 24 + 56) } as usize;
    let mut state = HookState {
        image_base: base,
        image_size,
        fields: Vec::new(),
        trace: None,
        script: None,
        log,
        last_tick: None,
        driving_tick: None,
        rows: 0,
    };
    state.log(&format!(
        "shuppatsu-hook: exe base {base:#x}, size {image_size:#x}"
    ));
    if let Some(path) = env_file("SHUPPATSU_HOOK_SCHEMA") {
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| schema::parse(&t))
        {
            Ok(fields) => state.fields = fields,
            Err(e) => state.log(&format!("schema {path}: {e}")),
        }
    }
    if let Some(path) = env_file("SHUPPATSU_HOOK_TRACE") {
        match File::create(&path) {
            Ok(f) => {
                let mut w = BufWriter::new(f);
                let _ = writeln!(w, "{}", schema::header(&state.fields));
                state.trace = Some(w);
            }
            Err(e) => state.log(&format!("trace {path}: {e}")),
        }
    }
    if let Some(path) = env_file("SHUPPATSU_HOOK_INPUTS") {
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| Script::parse(&t))
        {
            Ok(s) => state.script = Some(s),
            Err(e) => state.log(&format!("inputs {path}: {e}")),
        }
    }
    // SAFETY: base is our exe; the hook has the PeekMessageA signature.
    match unsafe {
        patch_iat(
            base,
            "user32.dll",
            "PeekMessageA",
            peek_message_hook as *const () as usize,
        )
    } {
        Some(orig) => {
            ORIGINAL_PEEK.store(orig, Ordering::Release);
            state.log("shuppatsu-hook: PeekMessageA redirected");
        }
        None => state.log("shuppatsu-hook: PeekMessageA import not found"),
    }
    if let Ok(mut g) = STATE.lock() {
        *g = Some(state);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: Hmodule, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        install();
    }
    1
}

/// Forwards to the system DirectInput (Wine's builtin when loaded by full path).
///
/// # Safety
/// Called by the game with the DirectInputCreateA contract.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DirectInputCreateA(
    instance: *mut c_void,
    version: u32,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    // SAFETY: loading the system DLL by absolute path and resolving a documented export.
    unsafe {
        let module = LoadLibraryA(c"C:\\windows\\system32\\dinput.dll".as_ptr().cast());
        if module.is_null() {
            return E_FAIL;
        }
        let proc = GetProcAddress(module, c"DirectInputCreateA".as_ptr().cast());
        if proc.is_null() {
            return E_FAIL;
        }
        let f: DirectInputCreateFn = std::mem::transmute::<*mut c_void, DirectInputCreateFn>(proc);
        f(instance, version, out, outer)
    }
}
