//! Hostile-input fuzzing of the running emulator.
//!
//! Random multi-function programs that call every built-in OS routine with
//! random argument counts, run against random RAM (including the pointer
//! registers, the heap headers and the keyboard), interleaved with every host
//! operation a script runner, panel or debugger can perform mid-run:
//! `set_var`, `set_ram`, `set_key`, `reset`, `step`, `run` with breakpoints,
//! and all the inspectors. The only assertion is "no panic, no hang": in wasm
//! a panic tears down the whole session.
//!
//! Run in debug (overflow checks on) by `cargo test`. Set `N2T_VM_FUZZ_ITERS`
//! to run longer.

use n2t_vm::{Breakpoints, BuiltinOs, Program, RunStatus, Segment, StepStatus, Vm, parse};

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
    /// A 16-bit value biased toward the interesting boundaries.
    fn word(&mut self) -> i16 {
        const EDGES: [i16; 24] = [
            0, 1, -1, 2, 5, 7, 8, 255, 256, 2047, 2048, 2049, 16382, 16383, 16384, 24575, 24576,
            24577, 32767, -32768, -2, 181, 182, 128,
        ];
        match self.below(4) {
            0 => *self.pick(&EDGES),
            1 => self.below(600) as i16,
            2 => (self.below(32768) as i16).wrapping_add(self.below(2) as i16 * -32768),
            _ => 2048 + self.below(14336) as i16,
        }
    }
    /// A non-negative command operand biased toward boundaries.
    fn operand(&mut self) -> u16 {
        const EDGES: [u16; 10] = [0, 1, 2, 7, 8, 238, 239, 2047, 16383, 32767];
        if self.below(3) == 0 {
            *self.pick(&EDGES)
        } else {
            self.below(12) as u16
        }
    }
}

const BUILTINS: [&str; 51] = [
    "Math.init",
    "Math.abs",
    "Math.multiply",
    "Math.divide",
    "Math.min",
    "Math.max",
    "Math.sqrt",
    "String.new",
    "String.dispose",
    "String.length",
    "String.charAt",
    "String.setCharAt",
    "String.appendChar",
    "String.eraseLastChar",
    "String.intValue",
    "String.setInt",
    "String.backSpace",
    "String.doubleQuote",
    "String.newLine",
    "Array.new",
    "Array.dispose",
    "Output.init",
    "Output.moveCursor",
    "Output.printChar",
    "Output.printString",
    "Output.printInt",
    "Output.println",
    "Output.backSpace",
    "Screen.init",
    "Screen.clearScreen",
    "Screen.setColor",
    "Screen.drawPixel",
    "Screen.drawLine",
    "Screen.drawRectangle",
    "Screen.drawCircle",
    "Keyboard.init",
    "Keyboard.keyPressed",
    "Keyboard.readChar",
    "Keyboard.readLine",
    "Keyboard.readInt",
    "Memory.init",
    "Memory.peek",
    "Memory.poke",
    "Memory.alloc",
    "Memory.deAlloc",
    "Sys.init",
    "Sys.halt",
    "Sys.error",
    "Sys.wait",
    // Not built-ins: resolved to user functions below.
    "Main.f1",
    "Other.g",
];

const SEGMENTS: [&str; 8] = [
    "constant", "local", "argument", "this", "that", "temp", "pointer", "static",
];

fn random_function(rng: &mut Rng, name: &str, out: &mut Vec<String>) {
    out.push(format!("function {name} {}", rng.below(6)));
    let n = 5 + rng.below(40);
    let mut labels = 0u64;
    for _ in 0..n {
        let line = match rng.below(16) {
            0..=3 => {
                // push of a boundary value, so calls get interesting arguments
                let v = rng.word();
                if v < 0 {
                    format!(
                        "push constant {}\nneg",
                        (v as i32).unsigned_abs().min(32767)
                    )
                } else {
                    format!("push constant {v}")
                }
            }
            4 | 5 => format!("push {} {}", rng.pick(&SEGMENTS), rng.operand()),
            6 => {
                let seg = *rng.pick(&SEGMENTS[1..]);
                format!("pop {seg} {}", rng.operand())
            }
            7 => rng
                .pick(&["add", "sub", "neg", "eq", "gt", "lt", "and", "or", "not"])
                .to_string(),
            8 => {
                labels += 1;
                format!("label L{labels}")
            }
            9 if labels > 0 => format!("if-goto L{}", 1 + rng.below(labels)),
            10 if labels > 0 && rng.below(4) == 0 => format!("goto L{}", 1 + rng.below(labels)),
            11..=13 => format!("call {} {}", rng.pick(&BUILTINS), rng.below(6)),
            14 => "return".to_string(),
            _ => "push constant 1".to_string(),
        };
        out.push(line);
    }
    out.push("push constant 0".to_string());
    out.push("return".to_string());
}

fn random_program(rng: &mut Rng) -> Program {
    let mut main = Vec::new();
    // Sometimes a user Sys.init (needs to live in Sys.vm), sometimes the
    // built-in boot through Main.main, sometimes a bare snippet.
    let boot = rng.below(3);
    let mut files = Vec::new();
    if boot == 0 {
        let mut sys = Vec::new();
        random_function(rng, "Sys.init", &mut sys);
        files.push(parse("Sys", &(sys.join("\n") + "\n")));
    }
    if boot == 2 {
        main.push("push constant 3".to_string());
        main.push("push constant 4".to_string());
        main.push("call Main.f1 2".to_string());
    }
    random_function(rng, "Main.main", &mut main);
    random_function(rng, "Main.f1", &mut main);
    let src = main.join("\n") + "\n";
    let mut other = Vec::new();
    random_function(rng, "Other.g", &mut other);
    files.push(parse("Main", &src));
    files.push(parse("Other", &(other.join("\n") + "\n")));
    Program::link(files).0
}

fn poke_inspectors(vm: &Vm) {
    let _ = vm.call_stack();
    let _ = vm.stack();
    let _ = vm.temp_slice();
    let _ = vm.screen_slice();
    for seg in Segment::ALL {
        let _ = vm.segment_base(seg);
        let _ = vm.segment_slice(seg, 40_000);
    }
    let _ = vm.static_slice("Main");
    let _ = vm.current_line();
    let _ = vm.current_file();
    let _ = vm.current_function();
    let _ = vm.output_cursor();
    for name in [
        "sp",
        "local",
        "argument",
        "this",
        "that",
        "line",
        "currentFunction",
    ] {
        let _ = vm.get_var(name, None);
        let _ = vm.get_var_text(name);
    }
    for name in ["RAM", "local", "argument", "this", "that", "temp"] {
        for i in [0i64, 1, 7, 8, 24576, 24577, -1, i64::MAX, i64::MIN] {
            let _ = vm.get_var(name, Some(i));
        }
    }
}

fn host_poke(rng: &mut Rng, vm: &mut Vm) {
    match rng.below(12) {
        0 => vm.set_ram(rng.below(5) as u16, rng.word()),
        1 => vm.set_ram(rng.below(24600) as u16, rng.word()),
        2 => vm.set_key(*rng.pick(&[0u16, 0, 65, 128, 129, 48, 45, 140, 65535])),
        3 => {
            let name = *rng.pick(&["sp", "local", "argument", "this", "that", "line"]);
            let v = *rng.pick(&[
                0i64,
                1,
                256,
                300,
                2047,
                2048,
                24576,
                24577,
                -1,
                70_000,
                i64::MAX,
                i64::MIN,
            ]);
            let _ = vm.set_var(name, None, v);
        }
        4 => {
            let name = *rng.pick(&["RAM", "local", "argument", "this", "that", "temp"]);
            let i = *rng.pick(&[0i64, 3, 8, 24576, 24577, -1, i64::MAX, i64::MIN]);
            let v = *rng.pick(&[0i64, -1, 65535, 65536, i64::MAX, i64::MIN]);
            let _ = vm.set_var(name, Some(i), v);
        }
        5 if rng.below(8) == 0 => vm.reset(),
        // Heap header and string/array object corruption.
        6 => vm.set_ram(2048 + rng.below(4) as u16, rng.word()),
        _ => {}
    }
}

#[test]
fn hostile_programs_ram_and_host_calls_never_panic() {
    let iters = std::env::var("N2T_VM_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(400);
    let mut rng = Rng(0x5EED_0F7E_57AB);
    for _ in 0..iters {
        let program = random_program(&mut rng);
        let mut vm = Vm::new(program);
        let mut bp = Breakpoints::new();
        for _ in 0..rng.below(4) {
            bp.insert(
                *rng.pick(&["Main", "Other", "Sys", "Nope"]),
                rng.below(60) as u32,
            );
        }
        // Random RAM before the first step, sometimes.
        if rng.below(2) == 0 {
            for _ in 0..rng.below(64) {
                host_poke(&mut rng, &mut vm);
            }
        }
        let mut now = 0u64;
        for _slice in 0..40 {
            host_poke(&mut rng, &mut vm);
            now += rng.below(50);
            if rng.below(3) == 0 {
                for _ in 0..rng.below(20) {
                    match vm.step(now) {
                        StepStatus::Halted | StepStatus::Error(_) => break,
                        _ => {}
                    }
                }
            } else {
                match vm.run(rng.below(3000), now, &bp) {
                    RunStatus::Breakpoint { .. } => {
                        let _ = vm.step(now);
                    }
                    RunStatus::Halted | RunStatus::Error(_) if rng.below(2) == 0 => {
                        // Keep driving a stopped machine: must stay stopped, not panic.
                        let _ = vm.run(100, now, &bp);
                        let _ = vm.step(now);
                    }
                    _ => {}
                }
            }
            poke_inspectors(&vm);
        }
    }
}

#[test]
fn every_builtin_executes_with_hostile_arguments_and_ram() {
    let mut rng = Rng(0x8F76_C726_A843);
    let bp = Breakpoints::new();
    for name in BUILTINS {
        let Some(builtin) = BuiltinOs::from_name(name) else {
            continue;
        };
        let arity = builtin.arity();
        let source = format!("call {name} {arity}\n");
        let (program, diagnostics) = Program::link(vec![parse("Probe", &source)]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let mut vm = Vm::new(program);
        for _ in 0..64 {
            vm.reset();
            // Set up a valid call frame so execution reaches the builtin.
            for index in 0..arity {
                vm.set_ram(256 + index, rng.word());
            }
            for addr in [1, 2, 3, 4, 2048, 2049, 2050, 2051, 24576] {
                vm.set_ram(addr, rng.word());
            }
            vm.set_var("sp", None, i64::from(256 + arity)).unwrap();
            let _ = vm.run(1, u64::MAX - 10, &bp);
            for key in [65, 0, 129, 0, 128, 0] {
                vm.set_key(key);
                let _ = vm.run(2, u64::MAX, &bp);
            }
        }
    }
}
