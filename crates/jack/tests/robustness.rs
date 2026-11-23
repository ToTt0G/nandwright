//! The compiler never panics and never overflows the stack, whatever it is fed.

use n2t_jack::{compile, compile_program, parse, resolve};

/// A small deterministic PRNG (xorshift64*), so the test needs no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SEED_PROGRAM: &str = include_str!("fixtures/LinkedList/List.jack");

const FRAGMENTS: &[&str] = &[
    "class", "Main", "{", "}", "(", ")", "[", "]", ";", ",", ".", "+", "-", "*", "/", "&", "|", "<",
    ">", "=", "~", "let", "do", "if", "else", "while", "return", "var", "int", "char", "boolean",
    "void", "true", "false", "null", "this", "field", "static", "constructor", "function",
    "method", "x", "y", "Output", "printInt", "0", "32767", "32768", "1.5", "\"str\"", "\"open",
    "/*", "*/", "//", "/**", "\n", "\r\n", "\t", " ", "\u{feff}", "é", "😀", "$", "'", "\\",
];

fn run_all(src: &str) {
    let p = parse(src);
    let _ = resolve(&p);
    let c = compile("Main", src);
    if c.vm.is_some() {
        assert!(!c.has_errors());
        assert_eq!(c.line_map.len(), c.vm.as_ref().unwrap().lines().count());
    }
    assert_eq!(c.diagnostics.len(), c.official_lines.len());
    for d in &c.diagnostics {
        if let Some(s) = d.span {
            assert!(s.start <= s.end && s.end <= src.len(), "{d:?}");
            assert!(src.is_char_boundary(s.start) && src.is_char_boundary(s.end), "{d:?}");
        }
        assert!(!d.message.contains('\n') && !d.message.contains("  "), "{}", d.message);
    }
}

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    // Smaller than the 1 MiB a wasm32 module gets by default.
    std::thread::Builder::new()
        .stack_size(768 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn random_token_soup() {
    on_small_stack(|| {
        let mut rng = Rng(0x1234_5678_9abc_def0);
        for _ in 0..1500 {
            let n = rng.below(80);
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(FRAGMENTS[rng.below(FRAGMENTS.len())]);
                if rng.below(3) == 0 {
                    s.push(' ');
                }
            }
            run_all(&s);
        }
    });
}

#[test]
fn mutated_programs() {
    on_small_stack(|| {
        let mut rng = Rng(42);
        let base: Vec<char> = SEED_PROGRAM.chars().collect();
        for _ in 0..1500 {
            let mut s = base.clone();
            for _ in 0..1 + rng.below(6) {
                let at = rng.below(s.len().max(1));
                match rng.below(4) {
                    0 => {
                        s.remove(at.min(s.len() - 1));
                    }
                    1 => {
                        let f = FRAGMENTS[rng.below(FRAGMENTS.len())];
                        for (k, c) in f.chars().enumerate() {
                            s.insert(at + k, c);
                        }
                    }
                    2 => s.truncate(at),
                    _ => {
                        let last = s.len() - 1;
                        let b = rng.below(s.len().max(1));
                        s.swap(at.min(last), b.min(last));
                    }
                }
                if s.is_empty() {
                    break;
                }
            }
            let text: String = s.into_iter().collect();
            run_all(&text);
        }
    });
}

#[test]
fn random_bytes() {
    on_small_stack(|| {
        let mut rng = Rng(7);
        for _ in 0..500 {
            let bytes: Vec<u8> = (0..rng.below(200)).map(|_| rng.next() as u8).collect();
            run_all(&String::from_utf8_lossy(&bytes));
        }
    });
}

#[test]
fn every_prefix_of_a_program() {
    on_small_stack(|| {
        let src = include_str!("fixtures/Sieve/Sieve.jack");
        for (i, _) in src.char_indices().step_by(3) {
            run_all(&src[..i]);
        }
    });
}

fn deep(open: &str, close: &str, n: usize) -> String {
    format!(
        "class Main {{ function int f() {{ var int x; let x = {}1{}; return x; }} }}",
        open.repeat(n),
        close.repeat(n)
    )
}

#[test]
fn ten_thousand_deep_expression_is_a_diagnostic() {
    on_small_stack(|| {
        for (open, close) in [("(", ")"), ("-", ""), ("~", ""), ("Main.f(", ")")] {
            let src = deep(open, close, 10_000);
            let c = compile("Main", &src);
            assert!(c.vm.is_none(), "{open}");
            assert!(
                c.diagnostics
                    .iter()
                    .any(|d| d.code.as_str() == "jack-nesting-too-deep"),
                "{open}"
            );
            run_all(&src);
        }
        let blocks = format!(
            "class Main {{ function void f() {{ var int x; {} return; {} }} }}",
            "while (x) { ".repeat(10_000),
            "}".repeat(10_000)
        );
        run_all(&blocks);
    });
}

#[test]
fn hundred_thousand_open_parentheses() {
    on_small_stack(|| {
        let src = format!(
            "class Main {{ function void f() {{ var int x; let x = {}; return; }} }}",
            "(".repeat(100_000)
        );
        let c = compile("Main", &src);
        assert!(c.has_errors());
        run_all(&src);
        let p = compile_program(&[("Main", &src), ("Other", "(((((((((")]);
        assert_eq!(p.files.len(), 2);
    });
}

#[test]
fn nesting_up_to_the_official_limit_compiles() {
    on_small_stack(|| {
        // 99 parentheses inside the `let` expression: 100 nested expressions,
        // which the official compiler still handles.
        let c = compile("Main", &deep("(", ")", 99));
        assert!(c.vm.is_some(), "{:?}", c.diagnostics);
        let c = compile("Main", &deep("(", ")", 100));
        assert!(c.vm.is_none());
        // Unary chains do not count towards that limit.
        let c = compile("Main", &deep("-", "", 120));
        assert!(c.vm.is_some(), "{:?}", c.diagnostics);
    });
}
