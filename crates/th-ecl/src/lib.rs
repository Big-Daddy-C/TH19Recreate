//! ECL virtual machine: the scripting language that drives enemies, bullet patterns,
//! spell cards and stage timelines.
//!
//! This crate implements the *core* interpreter (opcodes 0–99: control flow, the typed data
//! stack, variables, arithmetic, sub calls and threads). Everything game-specific (opcodes
//! 300 and up: movement, bullets, sprites, ...) is delegated to a [`Host`].
//!
//! Behaviour follows th19.exe's interpreter (`FUN_005687d0` and its helpers), reimplemented:
//! * each script thread ([`Context`]) has a clock; every frame it runs all instructions whose
//!   timestamp has been reached, then the clock advances by the game speed (normally 1);
//! * `wait(n)` moves the clock back by `n`;
//! * values live on a byte-addressed stack of 4-byte words; pushed values occupy two words
//!   (a type tag `i`/`f`, then the value), locals are plain words relative to a frame base;
//! * an instruction argument flagged as a variable reads a local (`id >= 0`, byte offset from
//!   the frame base), a stack entry without popping (`-100 <= id < 0`, `-1` = top entry) or a
//!   game variable supplied by the host (`id < -100`);
//! * after each instruction the stack is popped by the byte count stored in its header.

use std::collections::HashMap;

use th_formats::ecl::{EclFile, Instr};

/// Location of a sub: (file index, sub index) in a [`Program`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SubRef {
    pub file: usize,
    pub sub: usize,
}

/// A loaded ECL program: a main file plus everything it includes (`ECLI`), with all subs
/// addressable by name.
#[derive(Default)]
pub struct Program {
    pub files: Vec<(String, EclFile)>,
    by_name: HashMap<String, SubRef>,
}

impl Program {
    /// Load `main` and, recursively, every file it includes, using `read` to fetch bytes.
    pub fn load(
        main: &str,
        mut read: impl FnMut(&str) -> Option<Vec<u8>>,
    ) -> Result<Self, th_formats::Error> {
        let mut prog = Program::default();
        let mut queue = vec![main.to_string()];
        while let Some(name) = queue.pop() {
            if prog.files.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let Some(bytes) = read(&name) else { continue };
            let file = EclFile::parse(&bytes)?;
            queue.extend(file.ecli.iter().cloned());
            let idx = prog.files.len();
            for (si, s) in file.subs.iter().enumerate() {
                // Earlier files win, so the main file's subs shadow included ones.
                prog.by_name.entry(s.name.clone()).or_insert(SubRef { file: idx, sub: si });
            }
            prog.files.push((name, file));
        }
        Ok(prog)
    }

    pub fn sub(&self, name: &str) -> Option<SubRef> {
        self.by_name.get(name).copied()
    }

    pub fn instrs(&self, r: SubRef) -> &[Instr] {
        &self.files[r.file].1.subs[r.sub].instrs
    }

    pub fn sub_name(&self, r: SubRef) -> &str {
        &self.files[r.file].1.subs[r.sub].name
    }

    /// The `.anm` files listed by the file a sub lives in.
    pub fn anim_list(&self, r: SubRef) -> &[String] {
        &self.files[r.file].1.anim
    }
}

/// Game variables and opcodes 300+ are provided by the host (the enemy/stage system).
pub trait Host {
    fn get_int(&mut self, id: i32) -> i32;
    fn get_float(&mut self, id: i32) -> f32;
    fn set_int(&mut self, id: i32, value: i32);
    fn set_float(&mut self, id: i32, value: f32);
    /// Uniform random 32-bit value (the game's RNG).
    fn rand(&mut self) -> u32;
    /// Execute a non-core instruction. Read arguments through `ctx`.
    fn exec(&mut self, ctx: &mut Context, prog: &Program, ins: &Instr) -> Flow;
}

/// What the interpreter should do after an instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Next,
    /// Stop running this thread for the rest of the frame.
    Yield,
    /// The thread has finished (or its owner was deleted).
    End,
    /// Stop for this frame and run the same instruction again next frame (a host
    /// handler returning -1 in th19.exe, e.g. ins_520 while a boss is alive).
    Retry,
}

const STACK_BYTES: usize = 0x1000;

/// `TH_ECL_TRACE=1` prints every executed instruction to stderr.
fn trace_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TH_ECL_TRACE").is_some())
}
const TAG_INT: u8 = b'i';
const TAG_FLOAT: u8 = b'f';

#[derive(Clone, Debug)]
struct Frame {
    sub: SubRef,
    ip: usize,
    time: f32,
    sp: usize,
    base: usize,
}

/// One ECL thread.
#[derive(Clone, Debug)]
pub struct Context {
    pub sub: Option<SubRef>,
    pub ip: usize,
    pub time: f32,
    /// Id given to async threads started with `callAsyncId` (0 otherwise).
    pub async_id: i32,
    /// Paused threads don't run (`ins_18`/`ins_19`).
    pub paused: bool,
    /// `1 << difficulty`: instructions whose rank mask lacks this bit are skipped
    /// (advanced past without popping the stack), as in the game's loop.
    pub rank_bit: u8,
    words: Vec<u32>,
    tags: Vec<u8>,
    sp: usize,
    base: usize,
    calls: Vec<Frame>,
    /// `ins_91`/`ins_92` float interpolators, written to their variable every frame.
    interps: [VarInterp; 8],
    /// Core opcodes this implementation hasn't covered, with hit counts.
    pub unknown: HashMap<u16, u32>,
}

/// An interpolator bound to a variable. The variable is re-resolved every frame from the
/// instruction that started it, against the frame base active at that time.
#[derive(Clone, Debug, Default)]
struct VarInterp {
    interp: th_anm::interp::Interp<1>,
    var_id: f32,
    is_var: bool,
    base: usize,
}

/// A thread started by `callAsync`/`callAsyncId`, to be attached to the same owner.
pub struct Spawned(pub Context);

impl Context {
    pub fn new(sub: SubRef) -> Self {
        Self {
            sub: Some(sub),
            ip: 0,
            time: 0.0,
            async_id: 0,
            paused: false,
            rank_bit: 0xff,
            words: vec![0; STACK_BYTES / 4],
            tags: vec![0; STACK_BYTES / 4],
            sp: 0,
            base: 0,
            calls: Vec::new(),
            interps: Default::default(),
            unknown: HashMap::new(),
        }
    }

    pub fn finished(&self) -> bool {
        self.sub.is_none()
    }

    // ----- raw stack ------------------------------------------------------------

    fn word(&self, byte: usize) -> u32 {
        self.words.get(byte / 4).copied().unwrap_or(0)
    }
    fn set_word(&mut self, byte: usize, v: u32) {
        if let Some(w) = self.words.get_mut(byte / 4) {
            *w = v;
        }
    }
    fn push(&mut self, tag: u8, bits: u32) {
        if self.sp + 8 > STACK_BYTES {
            return;
        }
        self.tags[self.sp / 4] = tag;
        self.sp += 4;
        self.set_word(self.sp, bits);
        self.sp += 4;
    }
    /// Pop a tagged value, converting to the requested kind.
    fn pop_raw(&mut self) -> (u8, u32) {
        if self.sp < 8 {
            return (TAG_INT, 0);
        }
        self.sp -= 4;
        let bits = self.word(self.sp);
        self.sp -= 4;
        (self.tags[self.sp / 4], bits)
    }
    fn pop_int(&mut self) -> i32 {
        match self.pop_raw() {
            (TAG_FLOAT, b) => f32::from_bits(b) as i32,
            (_, b) => b as i32,
        }
    }
    fn pop_float(&mut self) -> f32 {
        match self.pop_raw() {
            (TAG_INT, b) => b as i32 as f32,
            (_, b) => f32::from_bits(b),
        }
    }
    fn push_int(&mut self, v: i32) {
        self.push(TAG_INT, v as u32);
    }
    fn push_float(&mut self, v: f32) {
        self.push(TAG_FLOAT, v.to_bits());
    }
    /// A stack entry addressed relative to the top (`-1` = top) without popping.
    fn peek(&self, rel: i32) -> (u8, u32) {
        let at = self.sp as i64 + rel as i64 * 8;
        if at < 0 {
            return (TAG_INT, 0);
        }
        let at = at as usize;
        (self.tags.get(at / 4).copied().unwrap_or(TAG_INT), self.word(at + 4))
    }

    // ----- argument access (for the core and for hosts) ---------------------------

    fn raw_arg(ins: &Instr, n: usize) -> u32 {
        ins.arg_i32(n) as u32
    }

    /// Integer argument `n`, resolving variables.
    pub fn int_arg(&self, ins: &Instr, n: usize, host: &mut dyn Host) -> i32 {
        self.int_at(ins, n, n, host)
    }

    /// Float argument `n`, resolving variables (float variable ids are stored as floats).
    pub fn float_arg(&self, ins: &Instr, n: usize, host: &mut dyn Host) -> f32 {
        self.float_at(ins, n, n, host)
    }

    /// Integer value stored at argument word `word`, a variable if mask bit `bit` is set.
    pub fn int_at(&self, ins: &Instr, word: usize, bit: usize, host: &mut dyn Host) -> i32 {
        let raw = Self::raw_arg(ins, word) as i32;
        if ins.param_mask & (1 << bit) == 0 {
            return raw;
        }
        self.read_int_var(raw, host)
    }

    pub fn float_at(&self, ins: &Instr, word: usize, bit: usize, host: &mut dyn Host) -> f32 {
        let raw = f32::from_bits(Self::raw_arg(ins, word));
        if ins.param_mask & (1 << bit) == 0 {
            return raw;
        }
        self.read_float_var(raw, host)
    }

    fn read_int_var(&self, id: i32, host: &mut dyn Host) -> i32 {
        if id >= 0 {
            self.word(self.base + id as usize) as i32
        } else if id >= -100 {
            match self.peek(id) {
                (TAG_FLOAT, b) => f32::from_bits(b) as i32,
                (_, b) => b as i32,
            }
        } else {
            host.get_int(id)
        }
    }

    fn read_float_var(&self, id: f32, host: &mut dyn Host) -> f32 {
        if id >= 0.0 {
            f32::from_bits(self.word(self.base + id as usize))
        } else if id <= -1.0 && id >= -100.0 {
            match self.peek(id as i32) {
                (TAG_INT, b) => b as i32 as f32,
                (_, b) => f32::from_bits(b),
            }
        } else {
            host.get_float(id as i32)
        }
    }

    /// Write an integer/float variable by id (as stored in an argument).
    pub fn write_int(&mut self, id: i32, v: i32, host: &mut dyn Host) {
        if id >= 0 {
            self.set_word(self.base + id as usize, v as u32);
        } else {
            host.set_int(id, v);
        }
    }

    pub fn write_float(&mut self, id: f32, v: f32, host: &mut dyn Host) {
        if id >= 0.0 {
            self.set_word(self.base + id as usize, v.to_bits());
        } else {
            host.set_float(id as i32, v);
        }
    }

    /// Write an integer variable named by argument `n`.
    pub fn set_int_var(&mut self, ins: &Instr, n: usize, v: i32, host: &mut dyn Host) {
        let id = Self::raw_arg(ins, n) as i32;
        if id >= 0 {
            self.set_word(self.base + id as usize, v as u32);
        } else {
            host.set_int(id, v);
        }
    }

    /// Write a float variable named by argument `n` (the id is stored as a float).
    pub fn set_float_var(&mut self, ins: &Instr, n: usize, v: f32, host: &mut dyn Host) {
        let id = f32::from_bits(Self::raw_arg(ins, n));
        if id >= 0.0 {
            self.set_word(self.base + id as usize, v.to_bits());
        } else {
            host.set_float(id as i32, v);
        }
    }

    /// Current value of a float variable named by argument `n` (for read-modify-write ops).
    fn get_float_var(&self, ins: &Instr, n: usize, host: &mut dyn Host) -> f32 {
        let id = f32::from_bits(Self::raw_arg(ins, n));
        if id >= 0.0 {
            f32::from_bits(self.word(self.base + id as usize))
        } else {
            host.get_float(id as i32)
        }
    }

    fn get_int_var(&self, ins: &Instr, n: usize, host: &mut dyn Host) -> i32 {
        let id = Self::raw_arg(ins, n) as i32;
        if id >= 0 {
            self.word(self.base + id as usize) as i32
        } else {
            host.get_int(id)
        }
    }

    // ----- sub calls --------------------------------------------------------------

    /// Decode a call's string argument (sub name) and its typed arguments, and write the
    /// arguments as the callee's first locals at `dest` (byte offset in `target`).
    /// Each argument entry is 8 bytes: a source tag (`i`/`f`/`g`...), the callee type
    /// (`i`/`f`) and padding, then the 4-byte value; argument 0 is the string.
    fn call_args(&mut self, ins: &Instr, host: &mut dyn Host) -> (String, Vec<(u8, u32)>) {
        let Some((name, used)) = ins.arg_str(0) else { return (String::new(), Vec::new()) };
        let mut args = Vec::new();
        let mut at = used;
        let mut param = 1usize;
        while at + 8 <= ins.args.len() {
            let src = ins.args[at];
            let dst = ins.args[at + 1];
            let raw = u32::from_le_bytes(ins.args[at + 4..at + 8].try_into().unwrap());
            let is_var = ins.param_mask & (1 << param) != 0;
            let value: (u8, u32) = if src == b'f' || src == b'g' {
                let mut f = f32::from_bits(raw);
                if is_var {
                    // Stack references are consumed when packing call arguments.
                    f = if f <= -1.0 && f >= -100.0 { self.pop_float() } else { self.read_float_var(f, host) };
                }
                if dst == b'f' { (TAG_FLOAT, f.to_bits()) } else { (TAG_INT, (f as i32) as u32) }
            } else {
                let mut v = raw as i32;
                if is_var {
                    v = if (-100..0).contains(&v) { self.pop_int() } else { self.read_int_var(v, host) };
                }
                if dst == b'f' { (TAG_FLOAT, (v as f32).to_bits()) } else { (TAG_INT, v as u32) }
            };
            args.push(value);
            at += 8;
            param += 1;
        }
        (name, args)
    }

    fn write_args(&mut self, at: usize, args: &[(u8, u32)]) {
        for (i, &(_, bits)) in args.iter().enumerate() {
            self.set_word(at + i * 4, bits);
        }
    }

    // ----- execution ----------------------------------------------------------------

    /// Run this thread for one frame. New async threads are appended to `spawned`;
    /// `kill_async(id)` requests are returned in `kills` (id 0 with `all` = kill all).
    pub fn run(&mut self, prog: &Program, host: &mut dyn Host, game_speed: f32, out: &mut Effects) -> Flow {
        if self.paused {
            return Flow::Next;
        }
        let mut budget = 20_000;
        loop {
            let Some(sub) = self.sub else { return Flow::End };
            let instrs = prog.instrs(sub);
            let Some(ins) = instrs.get(self.ip) else {
                // Falling off the end of a sub behaves like returning.
                if !self.ret() {
                    self.sub = None;
                    return Flow::End;
                }
                continue;
            };
            if ins.time as f32 > self.time {
                break;
            }
            budget -= 1;
            if budget == 0 {
                break;
            }
            if ins.rank_mask & self.rank_bit == 0 {
                self.ip += 1;
                continue;
            }
            let ip_before = (self.sub, self.ip);
            if trace_enabled() {
                let top: Vec<String> = (1..=3.min(self.sp / 8))
                    .map(|i| {
                        let (t, b) = self.peek(-(i as i32));
                        if t == TAG_FLOAT { format!("{}f", f32::from_bits(b)) } else { format!("{}", b as i32) }
                    })
                    .collect();
                eprintln!(
                    "[ecl] {} @{} t={} op={} sp={} base={} top={top:?}",
                    prog.sub_name(sub), ins.offset, self.time, ins.opcode, self.sp, self.base
                );
            }
            let flow = self.exec(prog, ins, host, out);
            match flow {
                Flow::End => {
                    self.sub = None;
                    return Flow::End;
                }
                Flow::Yield => {
                    self.finish_instr(ins, ip_before);
                    break;
                }
                Flow::Retry => break,
                Flow::Next => self.finish_instr(ins, ip_before),
            }
        }
        self.time += game_speed;
        self.update_interps(host);
        Flow::Next
    }

    fn update_interps(&mut self, host: &mut dyn Host) {
        for i in 0..self.interps.len() {
            let slot = &mut self.interps[i];
            let Some([v]) = slot.interp.step() else { continue };
            if slot.interp.end_time >= 0 && slot.interp.time >= slot.interp.end_time {
                slot.interp.active = false;
            }
            let (id, is_var, base) = (slot.var_id, slot.is_var, slot.base);
            if !is_var {
                continue;
            }
            if id >= 0.0 {
                self.set_word(base + id as usize, v.to_bits());
            } else {
                host.set_float(id as i32, v);
            }
        }
    }

    /// Post-instruction bookkeeping: pop consumed stack bytes and advance (unless jumped).
    fn finish_instr(&mut self, ins: &Instr, before: (Option<SubRef>, usize)) {
        let pop = (ins.stack_refs & 0xff) as usize;
        self.sp = self.sp.saturating_sub(pop);
        if (self.sub, self.ip) == before {
            self.ip += 1;
        }
    }

    fn jump(&mut self, prog: &Program, ins: &Instr, offset: i32, time: i32) {
        let Some(sub) = self.sub else { return };
        let target = (ins.offset as i64 + offset as i64) as u32;
        if let Some(i) = prog.files[sub.file].1.subs[sub.sub].index_of_offset(target) {
            self.ip = i;
            self.time = time as f32;
        } else {
            self.sub = None;
        }
    }

    /// Return from a sub. Returns false when there is nothing to return to.
    fn ret(&mut self) -> bool {
        match self.calls.pop() {
            Some(f) => {
                self.sub = Some(f.sub);
                self.ip = f.ip + 1;
                self.time = f.time;
                self.sp = f.sp;
                self.base = f.base;
                true
            }
            None => false,
        }
    }

    fn exec(&mut self, prog: &Program, ins: &Instr, host: &mut dyn Host, out: &mut Effects) -> Flow {
        use std::f32::consts::{PI, TAU};
        let wrap = |mut a: f32| {
            let mut n = 0;
            while a > PI && n < 32 {
                a -= TAU;
                n += 1;
            }
            while a < -PI && n < 64 {
                a += TAU;
                n += 1;
            }
            a
        };
        macro_rules! bin_int {
            ($f:expr) => {{
                let b = self.pop_int();
                let a = self.pop_int();
                let f: fn(i32, i32) -> i32 = $f;
                self.push_int(f(a, b));
            }};
        }
        macro_rules! bin_float {
            ($f:expr) => {{
                let b = self.pop_float();
                let a = self.pop_float();
                let f: fn(f32, f32) -> f32 = $f;
                self.push_float(f(a, b));
            }};
        }
        macro_rules! cmp_int {
            ($f:expr) => {{
                let b = self.pop_int();
                let a = self.pop_int();
                let f: fn(i32, i32) -> bool = $f;
                self.push_int(f(a, b) as i32);
            }};
        }
        macro_rules! cmp_float {
            ($f:expr) => {{
                let b = self.pop_float();
                let a = self.pop_float();
                let f: fn(f32, f32) -> bool = $f;
                self.push_int(f(a, b) as i32);
            }};
        }
        match ins.opcode {
            0 | 22 | 30 | 31 => {}
            // delete: this thread (and, for a main thread, its owner) ends.
            1 => return Flow::End,
            // return
            10 => {
                if !self.ret() {
                    return Flow::End;
                }
                return Flow::Next;
            }
            // call(sub, args...)
            11 => {
                let (name, args) = self.call_args(ins, host);
                let Some(target) = prog.sub(&name) else { return Flow::Next };
                self.calls.push(Frame { sub: self.sub.unwrap(), ip: self.ip, time: self.time, sp: self.sp, base: self.base });
                // The game stores a 4-word return record on the stack (saved sp, time, ip,
                // sub); the arguments go right after it and become the callee's first
                // locals once its stackAlloc makes that point the frame base. Keeping the
                // same layout keeps uninitialised locals reading the same stale memory.
                let sp = self.sp;
                self.set_word(sp, sp as u32);
                self.set_word(sp + 4, self.time.to_bits());
                self.set_word(sp + 8, ins.offset);
                self.set_word(sp + 12, self.sub.map_or(0, |s| s.sub as u32));
                self.sp = (sp + 16).min(STACK_BYTES - 8);
                self.write_args(self.sp, &args);
                self.sub = Some(target);
                self.ip = 0;
                self.time = 0.0;
                return Flow::Next;
            }
            // jmp(offset, time)
            12 => self.jump(prog, ins, ins.arg_i32(0), ins.arg_i32(1)),
            // jmpEq / jmpNeq: pop a value and jump if it is zero / non-zero.
            13 | 14 => {
                let v = self.pop_float();
                if (v == 0.0) == (ins.opcode == 13) {
                    self.jump(prog, ins, ins.arg_i32(0), ins.arg_i32(1));
                }
            }
            // callAsync(sub, args...) / callAsyncId(sub, id, args...)
            15 | 16 => {
                let (name, mut args) = self.call_args(ins, host);
                let Some(target) = prog.sub(&name) else { return Flow::Next };
                let mut child = Context::new(target);
                child.rank_bit = self.rank_bit;
                if ins.opcode == 16 && !args.is_empty() {
                    child.async_id = args.remove(0).1 as i32;
                }
                child.write_args(0, &args);
                out.spawned.push(Spawned(child));
            }
            // killAsync(id), pauseAsync(id), resumeAsync(id), setAsyncTime(id, t)
            17 => out.kill.push(self.int_arg(ins, 0, host)),
            18 => out.pause.push((self.int_arg(ins, 0, host), true)),
            19 => out.pause.push((self.int_arg(ins, 0, host), false)),
            21 => out.kill_all = true,
            // wait(n) / waitF(n): move the clock back.
            23 => self.time -= self.int_arg(ins, 0, host) as f32,
            24 => self.time -= self.float_arg(ins, 0, host),
            // stackAlloc(bytes): locals live at [base, base + bytes).
            40 => {
                let n = self.int_arg(ins, 0, host).max(0) as usize;
                let old_sp = self.sp;
                if old_sp + n + 4 <= STACK_BYTES {
                    self.sp = old_sp + n;
                    self.set_word(self.sp, self.base as u32);
                    self.sp += 4;
                    self.base = old_sp;
                }
            }
            41 => {
                let saved = self.word(self.sp.saturating_sub(4)) as usize;
                self.sp = self.base;
                self.base = saved;
            }
            // push(int): a stack-reference argument pops the top entry instead.
            42 => {
                let v = if ins.is_var(0) && (-100..0).contains(&ins.arg_i32(0)) {
                    self.pop_int()
                } else {
                    self.int_arg(ins, 0, host)
                };
                self.push_int(v);
            }
            43 => {
                let v = self.pop_int();
                self.set_int_var(ins, 0, v, host);
            }
            44 => {
                let id = ins.arg_f32(0);
                let v = if ins.is_var(0) && (-100.0..=-1.0).contains(&id) {
                    self.pop_float()
                } else {
                    self.float_arg(ins, 0, host)
                };
                self.push_float(v);
            }
            45 => {
                let v = self.pop_float();
                self.set_float_var(ins, 0, v, host);
            }
            50 => bin_int!(|a, b| a.wrapping_add(b)),
            51 => bin_float!(|a, b| a + b),
            52 => bin_int!(|a, b| a.wrapping_sub(b)),
            53 => bin_float!(|a, b| a - b),
            54 => bin_int!(|a, b| a.wrapping_mul(b)),
            55 => bin_float!(|a, b| a * b),
            56 => bin_int!(|a, b| a.checked_div(b).unwrap_or(0)),
            57 => bin_float!(|a, b| a / b),
            58 => bin_int!(|a, b| a.checked_rem(b).unwrap_or(0)),
            59 => cmp_int!(|a, b| a == b),
            60 => cmp_float!(|a, b| a == b),
            61 => cmp_int!(|a, b| a != b),
            62 => cmp_float!(|a, b| a != b),
            63 => cmp_int!(|a, b| a < b),
            64 => cmp_float!(|a, b| a < b),
            65 => cmp_int!(|a, b| a <= b),
            66 => cmp_float!(|a, b| a <= b),
            67 => cmp_int!(|a, b| a > b),
            68 => cmp_float!(|a, b| a > b),
            69 => cmp_int!(|a, b| a >= b),
            70 => cmp_float!(|a, b| a >= b),
            71 => {
                let a = self.pop_int();
                self.push_int((a == 0) as i32);
            }
            72 => {
                let a = self.pop_float();
                self.push_int((a == 0.0) as i32);
            }
            73 => cmp_int!(|a, b| a != 0 || b != 0),
            74 => cmp_int!(|a, b| a != 0 && b != 0),
            75 => bin_int!(|a, b| a ^ b),
            76 => bin_int!(|a, b| a | b),
            77 => bin_int!(|a, b| a & b),
            // dec(var): push the old value, then decrement the variable.
            78 => {
                let v = self.get_int_var(ins, 0, host);
                self.set_int_var(ins, 0, v.wrapping_sub(1), host);
                self.push_int(v);
            }
            79 => {
                let a = self.pop_float();
                self.push_float(a.sin());
            }
            80 => {
                let a = self.pop_float();
                self.push_float(a.cos());
            }
            // circlePos(&x, &y, angle, radius)
            81 => {
                let ang = wrap(self.float_arg(ins, 2, host));
                let r = self.float_arg(ins, 3, host);
                self.set_float_var(ins, 0, ang.cos() * r, host);
                self.set_float_var(ins, 1, ang.sin() * r, host);
            }
            // validRad(&angle)
            82 => {
                let a = self.get_float_var(ins, 0, host);
                self.set_float_var(ins, 0, wrap(a), host);
            }
            83 => {
                let a = self.pop_int();
                self.push_int(a.wrapping_neg());
            }
            84 => {
                let a = self.pop_float();
                self.push_float(-a);
            }
            // hypotSq / hypot (&out, x, y)
            85 | 86 => {
                let (x, y) = (self.float_arg(ins, 1, host), self.float_arg(ins, 2, host));
                let d2 = x * x + y * y;
                self.set_float_var(ins, 0, if ins.opcode == 86 { d2.sqrt() } else { d2 }, host);
            }
            // angle (&out, x1, y1, x2, y2): direction from point 1 to point 2
            87 => {
                let (x1, y1) = (self.float_arg(ins, 1, host), self.float_arg(ins, 2, host));
                let (x2, y2) = (self.float_arg(ins, 3, host), self.float_arg(ins, 4, host));
                self.set_float_var(ins, 0, (y2 - y1).atan2(x2 - x1), host);
            }
            // sqrt of the stack top
            88 => {
                let a = self.pop_float();
                self.push_float(a.sqrt());
            }
            // angleDiff (&out, a, b): a - b brought within one turn
            89 => {
                let a = self.float_arg(ins, 1, host);
                let mut b = self.float_arg(ins, 2, host);
                let d = if a - b > PI {
                    a - (b + TAU)
                } else {
                    if b - a > PI {
                        b -= TAU;
                    }
                    a - b
                };
                self.set_float_var(ins, 0, d, host);
            }
            // rotate (&x, &y, x, y, angle)
            90 => {
                let (x, y) = (self.float_arg(ins, 2, host), self.float_arg(ins, 3, host));
                let (s, c) = wrap(self.float_arg(ins, 4, host)).sin_cos();
                self.set_float_var(ins, 0, x * c - y * s, host);
                self.set_float_var(ins, 1, x * s + y * c, host);
            }
            // floatInterp(slot, &var, time, mode, from, to [, bezier1, bezier2])
            91 | 92 => {
                let slot = self.int_arg(ins, 0, host);
                let Some(_) = usize::try_from(slot).ok().filter(|&s| s < 8) else { return Flow::Next };
                let time = self.int_arg(ins, 2, host);
                let mode = self.int_arg(ins, 3, host);
                let from = self.float_arg(ins, 4, host);
                let to = self.float_arg(ins, 5, host);
                self.set_float_var(ins, 1, from, host);
                let mut it = th_anm::interp::Interp::<1>::default();
                if ins.opcode == 92 {
                    let (b1, b2) = (self.float_arg(ins, 6, host), self.float_arg(ins, 7, host));
                    it.start_bezier([from], [b1], [to], [b2], time);
                } else {
                    it.start([from], [to], time, mode);
                }
                self.interps[slot as usize] = VarInterp { interp: it, var_id: ins.arg_f32(1), is_var: ins.is_var(1), base: self.base };
            }
            // randomCircle(&x, &y, r1, r2): random point at a random radius between r2 and r1
            93 => {
                let r1 = self.float_arg(ins, 2, host);
                let r2 = self.float_arg(ins, 3, host);
                let u = host.rand() as f32 / u32::MAX as f32 * 2.0 - 1.0;
                let r = (r1 - r2) * u + r2;
                let ang = (host.rand() as f32 / u32::MAX as f32 * 2.0 - 1.0) * PI;
                self.set_float_var(ins, 0, ang.cos() * r, host);
                self.set_float_var(ins, 1, ang.sin() * r, host);
            }
            // ellipse(&x, &y, angle, radius, rotation, ratio)
            94 => {
                let ang = self.float_arg(ins, 2, host);
                let r = self.float_arg(ins, 3, host);
                let rot = self.float_arg(ins, 4, host);
                let ratio = self.float_arg(ins, 5, host);
                let t = wrap(ang - rot);
                let (px, py) = (t.cos() * r * ratio, t.sin() * r);
                let (s, c) = rot.sin_cos();
                self.set_float_var(ins, 0, c * px - s * py, host);
                self.set_float_var(ins, 1, s * px + c * py, host);
            }
            // reflectAngle(&out, angle, axis) = 2 * axis - angle
            95 => {
                let a = wrap(self.float_arg(ins, 1, host));
                let axis = wrap(self.float_arg(ins, 2, host));
                self.set_float_var(ins, 0, axis * 2.0 - a, host);
            }
            // xSign(&out, angle): -1 when the angle points left, +1 when it points right
            96 => {
                let a = wrap(self.float_arg(ins, 1, host));
                let left = a <= -std::f32::consts::FRAC_PI_2 || a > std::f32::consts::FRAC_PI_2;
                self.set_float_var(ins, 0, if left { -1.0 } else { 1.0 }, host);
            }
            // sign of an angle
            97 => {
                let a = wrap(self.float_arg(ins, 1, host));
                self.set_float_var(ins, 0, if a >= 0.0 { 1.0 } else { -1.0 }, host);
            }
            op if op >= 300 => return host.exec(self, prog, ins),
            op => *self.unknown.entry(op).or_default() += 1,
        }
        Flow::Next
    }
}

/// Sequential reader over an instruction's arguments, for hosts. Tracks the word offset and
/// the parameter index separately, since a string parameter spans several words but only
/// one bit of the variable mask.
pub struct Args<'a> {
    pub ins: &'a Instr,
    word: usize,
    param: usize,
}

impl<'a> Args<'a> {
    pub fn new(ins: &'a Instr) -> Self {
        Self { ins, word: 0, param: 0 }
    }
    pub fn int(&mut self, ctx: &Context, host: &mut dyn Host) -> i32 {
        let v = ctx.int_at(self.ins, self.word, self.param, host);
        self.word += 1;
        self.param += 1;
        v
    }
    pub fn float(&mut self, ctx: &Context, host: &mut dyn Host) -> f32 {
        let v = ctx.float_at(self.ins, self.word, self.param, host);
        self.word += 1;
        self.param += 1;
        v
    }
    /// The raw id of a variable parameter (for output arguments), as int and float.
    pub fn var(&mut self) -> (i32, f32) {
        let r = (self.ins.arg_i32(self.word), self.ins.arg_f32(self.word));
        self.word += 1;
        self.param += 1;
        r
    }
    pub fn string(&mut self) -> String {
        let (s, used) = self.ins.arg_str(self.word * 4).unwrap_or_default();
        self.word += used / 4;
        self.param += 1;
        s
    }
}

/// Side effects of running a thread that its owner must apply.
#[derive(Default)]
pub struct Effects {
    pub spawned: Vec<Spawned>,
    pub kill: Vec<i32>,
    pub kill_all: bool,
    pub pause: Vec<(i32, bool)>,
}

/// All threads belonging to one owner (an enemy, or the stage timeline).
#[derive(Default)]
pub struct Threads {
    pub main: Option<Context>,
    pub asyncs: Vec<Context>,
}

impl Threads {
    pub fn new(main: Context) -> Self {
        Self { main: Some(main), asyncs: Vec::new() }
    }

    /// Run every thread for one frame. Returns `Flow::End` when the main thread ended.
    pub fn run(&mut self, prog: &Program, host: &mut dyn Host, game_speed: f32) -> Flow {
        let mut fx = Effects::default();
        let mut main_ended = false;
        if let Some(m) = &mut self.main {
            if m.run(prog, host, game_speed, &mut fx) == Flow::End {
                main_ended = true;
            }
        }
        let mut i = 0;
        while i < self.asyncs.len() {
            let flow = self.asyncs[i].run(prog, host, game_speed, &mut fx);
            if flow == Flow::End {
                self.asyncs.remove(i);
            } else {
                i += 1;
            }
        }
        self.apply(fx);
        if main_ended {
            self.main = None;
            return Flow::End;
        }
        Flow::Next
    }

    fn apply(&mut self, fx: Effects) {
        if fx.kill_all {
            self.asyncs.clear();
        }
        for id in fx.kill {
            self.asyncs.retain(|c| c.async_id != id);
        }
        for (id, paused) in fx.pause {
            for c in self.asyncs.iter_mut().filter(|c| c.async_id == id) {
                c.paused = paused;
            }
        }
        // New threads start running on the next frame.
        self.asyncs.extend(fx.spawned.into_iter().map(|s| s.0));
    }
}
