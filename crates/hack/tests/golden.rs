//! Golden files and behaviour of the original example programs in
//! `tests/fixtures`. Regenerate the `.hack` files with `UPDATE_GOLDEN=1`;
//! `conformance/hack` checks them against the official assembler.

use std::path::{Path, PathBuf};

use n2t_hack::{Cpu, RunStatus, assemble, disassemble, hack_text, parse_hack};

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "asm"))
        .collect();
    v.sort();
    v
}

fn load(name: &str) -> Cpu {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let a = assemble(&std::fs::read_to_string(path).unwrap());
    assert!(a.is_ok(), "{name}: {:?}", a.diagnostics);
    let mut cpu = Cpu::new();
    cpu.load_rom(&a.words);
    cpu
}

/// Runs until the PC reaches `halt` (the program's final `@END 0;JMP` loop).
fn run_to(cpu: &mut Cpu, halt: u16, max: u64) {
    assert_eq!(cpu.run(max, &[halt]), RunStatus::Breakpoint(halt));
}

fn label(name: &str, label: &str) -> u16 {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let a = assemble(&std::fs::read_to_string(path).unwrap());
    a.symbols.iter().find(|s| s.name == label).unwrap().address
}

#[test]
fn golden_hack_files() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    for asm in fixtures() {
        let source = String::from_utf8_lossy(&std::fs::read(&asm).unwrap()).into_owned();
        let assembly = assemble(&source);
        assert!(
            assembly.is_ok(),
            "{}: {:?}",
            asm.display(),
            assembly.diagnostics
        );
        let text = hack_text(&assembly.words);
        let golden = asm.with_extension("hack");
        if update {
            std::fs::write(&golden, &text).unwrap();
        }
        let expected = std::fs::read_to_string(&golden)
            .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_GOLDEN=1", golden.display()));
        assert_eq!(text, expected, "{}", asm.display());
        assert_eq!(parse_hack(&expected).unwrap(), assembly.words);

        // Disassembly round trip for every word that has a mnemonic.
        for &w in &assembly.words {
            let d = disassemble(w);
            if !d.starts_with('?') {
                assert_eq!(assemble(&d).words, [w], "{d}");
            }
        }
    }
}

#[test]
fn countdown() {
    let mut cpu = load("Countdown.asm");
    cpu.set_ram(0, 5);
    run_to(&mut cpu, label("Countdown.asm", "DONE"), 10_000);
    assert_eq!(&cpu.ram_slice()[100..106], [5, 4, 3, 2, 1, 0]);
    assert_eq!((cpu.ram(0), cpu.ram(1)), (0, 5));
}

#[test]
fn sum_array() {
    let mut cpu = load("SumArray.asm");
    let values = [3u16, 0xFFFF, 1000, 7, 0x8000];
    for (i, &v) in values.iter().enumerate() {
        cpu.set_ram(100 + i as u16, v);
    }
    cpu.set_ram(0, values.len() as u16);
    run_to(&mut cpu, label("SumArray.asm", "END"), 10_000);
    let expected = values.iter().fold(0u16, |s, &v| s.wrapping_add(v));
    assert_eq!(cpu.ram(1), expected);
}

#[test]
fn diagonal() {
    let mut cpu = load("Diagonal.asm");
    run_to(&mut cpu, label("Diagonal.asm", "END"), 100_000);
    let screen = cpu.screen();
    for row in 0..256usize {
        for col in 0..512usize {
            let bit = screen[row * 32 + col / 16] >> (col % 16) & 1;
            assert_eq!(bit == 1, row == col, "pixel ({row}, {col})");
        }
    }
    assert!(cpu.take_screen_dirty());
}

#[test]
fn keylog() {
    let mut cpu = load("KeyLog.asm");
    let halt = label("KeyLog.asm", "HALT");
    cpu.set_ram(0, 3);
    for key in [65u16, 0, 0, 66, 66, 0, 140] {
        cpu.set_key(key);
        assert_eq!(cpu.run(200, &[halt]), RunStatus::Budget);
    }
    cpu.set_key(0);
    run_to(&mut cpu, halt, 1000);
    assert_eq!(&cpu.ram_slice()[200..204], [65, 66, 140, 0]);
    assert_eq!(cpu.ram(1), 3);
}

#[test]
fn fibonacci() {
    let mut cpu = load("Fibonacci.asm");
    cpu.set_ram(0, 30);
    run_to(&mut cpu, label("Fibonacci.asm", "END"), 100_000);
    let (mut a, mut b) = (0u16, 1u16);
    for i in 0..30 {
        assert_eq!(cpu.ram(100 + i), a, "fib({i})");
        (a, b) = (b, a.wrapping_add(b));
    }
    assert_eq!(cpu.ram(130), 0);
}

#[test]
fn bubble_sort() {
    let mut seed = 99u32;
    for n in [0usize, 1, 2, 7, 40] {
        let mut cpu = load("BubbleSort.asm");
        let mut values = Vec::new();
        for i in 0..n {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let v = ((seed >> 16) % 2000) as i16 - 1000;
            values.push(v);
            cpu.set_ram(100 + i as u16, v as u16);
        }
        cpu.set_ram(0, n as u16);
        run_to(&mut cpu, label("BubbleSort.asm", "END"), 10_000_000);
        values.sort();
        let got: Vec<i16> = cpu.ram_slice()[100..100 + n]
            .iter()
            .map(|&v| v as i16)
            .collect();
        assert_eq!(got, values);
    }
}

#[test]
fn quirks_assemble_like_java() {
    // Values observed with the official assembler for the same lines.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Quirks.asm");
    let a = assemble(&std::fs::read_to_string(path).unwrap());
    assert!(a.is_ok());
    assert_eq!(a.words[0], 12);
    assert_eq!(a.words[8], 0xFFFF);
    assert_eq!(a.words[11], 16, "@32768 is a variable");
    let sp = a.symbols.iter().find(|s| s.name == "SP").unwrap();
    assert_eq!(sp.address, 15, "(SP) redefines SP");
    let dup = a.symbols.iter().find(|s| s.name == "DUP").unwrap();
    assert_eq!(dup.address, 18, "last definition wins");
    assert_eq!(dup.other_definitions.len(), 1);
    assert_eq!(a.words[18], 15, "@ R 1 5 is R15");
    // Variables: 32768 -> 16, 1st -> 17, a+b -> 18, x.y$z:w_1 -> 19, Δx -> 20.
    assert_eq!(a.words[19], 20);
}
