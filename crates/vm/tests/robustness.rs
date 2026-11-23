//! Robustness / fuzz-style tests: thousands of pseudo-random and mutated `.vm`
//! inputs through `parse` and `Program::link`, plus random valid VM programs
//! through `Vm::run` with a step budget, asserting no panic and no hang.

use n2t_vm::{Breakpoints, Program, Vm, parse};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn range(&mut self, n: usize) -> usize {
        (self.next_u64() as usize) % n
    }
}

#[test]
fn parse_and_link_never_panic_on_random_and_mutated_inputs() {
    let mut rng = Rng::new(0x00C0_FFEE_1234_5678);
    let tokens = [
        "push",
        "pop",
        "add",
        "sub",
        "neg",
        "eq",
        "gt",
        "lt",
        "and",
        "or",
        "not",
        "label",
        "goto",
        "if-goto",
        "function",
        "call",
        "return",
        "constant",
        "local",
        "argument",
        "this",
        "that",
        "temp",
        "pointer",
        "static",
        "0",
        "1",
        "2",
        "7",
        "8",
        "238",
        "239",
        "240",
        "32767",
        "32768",
        "-1",
        "+5",
        "999999999999999999",
        "Foo.bar",
        "Sys.init",
        "Main.main",
        "Math.multiply",
        "L1",
        "L2",
        "//",
        "/*",
        "*/",
        "\n",
        "\r\n",
        "\t",
        " ",
        "λ",
        "█",
        "\u{FEFF}",
        "PUSH",
        "badseg",
        "a.b.c",
        ".",
        "",
    ];

    // 2,500 random token soup inputs
    for i in 0..2_500 {
        let mut src = String::new();
        let n_toks = rng.range(40);
        for _ in 0..n_toks {
            src.push_str(tokens[rng.range(tokens.len())]);
            if rng.range(4) == 0 {
                src.push('\n');
            } else {
                src.push(' ');
            }
        }
        let stem = if i % 3 == 0 { "Sys" } else { "Foo" };
        let parsed = parse(stem, &src);
        let (prog, _diags) = Program::link(vec![parsed]);
        let mut vm = Vm::new(prog);
        let _ = vm.run(200, 0, &Breakpoints::new());
    }

    // 1,000 mutated copies of a valid multi-function program
    let seed_prog = "\
function Foo.calc 2
push constant 10
pop local 0
label LOOP
push local 0
push constant 1
sub
pop local 0
push local 0
if-goto LOOP
call Math.abs 1
return
";
    for _ in 0..1_000 {
        let mut bytes = seed_prog.as_bytes().to_vec();
        let edits = 1 + rng.range(8);
        for _ in 0..edits {
            match rng.range(4) {
                0 if !bytes.is_empty() => {
                    let pos = rng.range(bytes.len());
                    bytes[pos] = (rng.next_u64() & 0x7F) as u8;
                }
                1 if !bytes.is_empty() => {
                    let pos = rng.range(bytes.len());
                    bytes.remove(pos);
                }
                2 => {
                    let pos = rng.range(bytes.len() + 1);
                    bytes.insert(pos, (rng.next_u64() & 0x7F) as u8);
                }
                _ => {
                    let pos = rng.range(bytes.len() + 1);
                    let snippet = tokens[rng.range(tokens.len())].as_bytes();
                    bytes.splice(pos..pos, snippet.iter().copied());
                }
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        let parsed = parse("Foo", &text);
        let (prog, _diags) = Program::link(vec![parsed]);
        let mut vm = Vm::new(prog);
        let _ = vm.run(200, 0, &Breakpoints::new());
    }
}

#[test]
fn random_valid_programs_never_panic_or_hang() {
    let mut rng = Rng::new(0xDEAD_BEEF_9876_5432);
    let bp = Breakpoints::new();

    for _ in 0..500 {
        let mut lines = vec![
            "function Main.main 4".to_string(),
            "push constant 2050".to_string(),
            "pop pointer 0".to_string(),
            "push constant 16384".to_string(),
            "pop pointer 1".to_string(),
        ];
        let n_cmds = 10 + rng.range(60);
        let mut labels_defined = 0usize;
        for _ in 0..n_cmds {
            let cmd = match rng.range(20) {
                0..=3 => format!("push constant {}", rng.range(1000)),
                4 => format!("push local {}", rng.range(4)),
                5 => format!("pop local {}", rng.range(4)),
                6 => format!("push temp {}", rng.range(8)),
                7 => format!("pop temp {}", rng.range(8)),
                8 => format!("push static {}", rng.range(10)),
                9 => format!("pop static {}", rng.range(10)),
                10 => format!("push this {}", rng.range(16)),
                11 => format!("pop that {}", rng.range(16)),
                12 => ["add", "sub", "neg", "eq", "gt", "lt", "and", "or", "not"][rng.range(9)]
                    .to_string(),
                13 => {
                    labels_defined += 1;
                    format!("label L{labels_defined}")
                }
                14 if labels_defined > 0 => format!("if-goto L{}", 1 + rng.range(labels_defined)),
                15 => "call Math.multiply 2".to_string(),
                16 => "call Math.divide 2".to_string(),
                17 => "call Math.sqrt 1".to_string(),
                18 => "call Memory.alloc 1".to_string(),
                _ => "call Screen.drawPixel 2".to_string(),
            };
            lines.push(cmd);
        }
        lines.push("push constant 0".to_string());
        lines.push("return".to_string());

        let src = lines.join("\n") + "\n";
        let parsed = parse("Main", &src);
        assert!(
            !parsed.has_errors(),
            "generated invalid vm:\n{src}\n{:?}",
            parsed.diagnostics
        );
        let (prog, diags) = Program::link(vec![parsed]);
        assert!(diags.iter().all(|d| !d.is_error()));

        let mut vm = Vm::new(prog);
        // Must terminate within budget (Budget, Halted, or Error — never panic or hang)
        let _status = vm.run(2_000, 0, &bp);
    }
}
