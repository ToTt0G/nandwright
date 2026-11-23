//! Measures emulation speed in instructions per second.
//!
//! ```text
//! cargo run -p n2t-hack --release --example bench [PROG.asm] [INSTRUCTIONS]
//! ```
//!
//! Without arguments it runs `tests/fixtures/Speed.asm`. The program should
//! not halt with an error (an endless loop is fine).

use std::time::Instant;

use n2t_hack::{Cpu, RunStatus, assemble};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = match args.first() {
        Some(p) => String::from_utf8_lossy(&std::fs::read(p).expect("readable file")).into_owned(),
        None => include_str!("../tests/fixtures/Speed.asm").to_string(),
    };
    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1_000_000_000);
    let assembly = assemble(&source);
    assert!(assembly.is_ok(), "{:?}", assembly.diagnostics);
    let mut cpu = Cpu::new();
    cpu.load_rom(&assembly.words);
    for (label, breakpoints) in [("no breakpoints", &[][..]), ("3 breakpoints", &[30000, 30001, 30002][..])] {
        let start = Instant::now();
        // Slices of 10M, as a host time-slicing the emulator would call it.
        let mut done = 0;
        while done < n {
            match cpu.run(10_000_000.min(n - done), breakpoints) {
                RunStatus::Budget => {}
                other => panic!("stopped: {other:?}"),
            }
            done += 10_000_000.min(n - done);
        }
        let secs = start.elapsed().as_secs_f64();
        println!(
            "{label}: {n} instructions in {secs:.2} s = {:.0} million/s",
            n as f64 / secs / 1e6
        );
    }
}
