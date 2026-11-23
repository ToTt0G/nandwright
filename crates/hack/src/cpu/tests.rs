use super::*;
use crate::asm::assemble;

fn cpu_with(src: &str) -> Cpu {
    let a = assemble(src);
    assert!(a.is_ok(), "{:?}", a.diagnostics);
    let mut cpu = Cpu::new();
    cpu.load_rom(&a.words);
    cpu
}

#[test]
fn am_writes_m_at_the_old_a() {
    let mut cpu = cpu_with("@100\nAMD=M+1\n");
    cpu.set_ram(100, 41);
    cpu.run(2, &[]);
    assert_eq!(
        (cpu.a(), cpu.d(), cpu.ram(100), cpu.ram(42)),
        (42, 42, 42, 0)
    );
}

#[test]
fn jumps_go_to_the_new_a() {
    let mut cpu = cpu_with("@5\nA=D;JMP\n");
    cpu.set_d(9);
    cpu.run(2, &[]);
    assert_eq!(cpu.pc(), 9);
}

#[test]
fn undocumented_comps_reuse_the_last_documented_alu_function() {
    let c = |comp: u8| crate::isa::encode_c(comp, 0b010, 0);
    let (u0, u1) = (0b0000001, 0b1000001); // undocumented, a = 0 and a = 1
    let mut cpu = Cpu::new();
    // Rows observed in the Java emulator with A=100, D=12, RAM[100]=10.
    let run = |cpu: &mut Cpu, word: u16| {
        cpu.set_rom(0, word);
        cpu.set_pc(0);
        cpu.set_a(100);
        cpu.set_d(12);
        cpu.set_ram(100, 10);
        cpu.step().unwrap();
        cpu.d() as i16
    };
    assert_eq!(run(&mut cpu, c(u0)), 4, "fresh: D&A");
    assert_eq!(run(&mut cpu, c(u1)), 8, "fresh: D&M");
    assert_eq!(run(&mut cpu, c(0b1000111)), -2, "M-D");
    assert_eq!(run(&mut cpu, c(u0)), 88, "latched c-bits with a = 0: A-D");
    assert_eq!(run(&mut cpu, c(u1)), -2, "latched c-bits with a = 1: M-D");
    assert_eq!(run(&mut cpu, 5), 12, "A-instructions keep the latch");
    assert_eq!(run(&mut cpu, c(u0)), 88);
    assert_eq!(
        run(&mut cpu, crate::isa::encode_c(0b0111010, 0, 0)),
        12,
        "-1, no dest"
    );
    cpu.load_rom(&[]);
    assert_eq!(run(&mut cpu, c(u0)), -1, "load keeps the latch");
}

#[test]
fn memory_checks_only_when_m_is_used() {
    let mut cpu = cpu_with("@24577\nD=A\nD=D+1\nA=1\n");
    assert_eq!(cpu.run(4, &[]), RunStatus::Budget);
    assert_eq!((cpu.a(), cpu.d()), (1, 24578));

    let mut cpu = cpu_with("@24577\nD=M\n");
    let status = cpu.run(10, &[]);
    assert_eq!(
        status,
        RunStatus::Error(CpuError::IllegalRead {
            pc: 1,
            address: 24577
        })
    );
    assert_eq!(cpu.pc(), 1);
    assert_eq!(cpu.cycles(), 1);

    let mut cpu = cpu_with("@24577\nAM=1\n");
    assert_eq!(
        cpu.run(10, &[]),
        RunStatus::Error(CpuError::IllegalWrite {
            pc: 1,
            address: 24577
        })
    );
    assert_eq!(cpu.a(), 24577, "a failed instruction changes nothing");

    let mut cpu = cpu_with("@24576\nM=1\nD=M\n");
    cpu.run(3, &[]);
    assert_eq!(
        (cpu.key(), cpu.d()),
        (1, 1),
        "the keyboard register is writable"
    );
}

#[test]
fn jump_target_and_end_of_rom() {
    let mut cpu = cpu_with("A=-1;JMP\n");
    assert_eq!(
        cpu.run(1, &[]),
        RunStatus::Error(CpuError::IllegalJump {
            pc: 0,
            target: 0xFFFF
        })
    );
    let mut cpu = cpu_with("@24577\n0;JMP\n");
    assert_eq!(cpu.run(2, &[]), RunStatus::Budget);
    assert_eq!(cpu.pc(), 24577);

    // Running off the program executes empty words up to the end of ROM.
    let mut cpu = cpu_with("@5\n");
    assert_eq!(cpu.run(32767, &[]), RunStatus::Budget);
    assert_eq!((cpu.pc(), cpu.a()), (32767, 5));
    assert_eq!(cpu.step(), Err(CpuError::PcOverflow { pc: 32767 }));

    let mut cpu = Cpu::new();
    cpu.set_rom(32767, 0xEA87); // 0;JMP
    cpu.set_pc(32767);
    assert_eq!(cpu.step(), Ok(()));
    assert_eq!(cpu.pc(), 0);
}

#[test]
fn illegal_words_fail_when_executed() {
    let mut cpu = Cpu::new();
    cpu.load_rom(&[1, 2, 0x8C10]);
    assert_eq!(
        cpu.run(5, &[]),
        RunStatus::Error(CpuError::IllegalInstruction {
            pc: 2,
            word: 0x8C10
        })
    );
    assert_eq!(cpu.a(), 2);
    cpu.load_rom(&[4, 0x8000, 0x8000]);
    assert_eq!(cpu.run(3, &[]), RunStatus::Budget);
    assert_eq!((cpu.a(), cpu.pc()), (4, 3));
}

#[test]
fn jump_conditions() {
    for (jump, taken) in [
        ("JGT", [false, false, true]),
        ("JEQ", [false, true, false]),
        ("JGE", [false, true, true]),
        ("JLT", [true, false, false]),
        ("JNE", [true, false, true]),
        ("JLE", [true, true, false]),
        ("JMP", [true, true, true]),
    ] {
        for (i, d) in [0xFFFFu16, 0, 1].into_iter().enumerate() {
            let mut cpu = cpu_with(&format!("@100\nD;{jump}\n"));
            cpu.set_d(d);
            cpu.run(2, &[]);
            assert_eq!(cpu.pc() == 100, taken[i], "{jump} with D={d}");
        }
    }
}

#[test]
fn breakpoints_and_budget() {
    let mut cpu = cpu_with("(LOOP)\n@1\nD=D+A\n@LOOP\n0;JMP\n");
    assert_eq!(cpu.run(1000, &[2]), RunStatus::Breakpoint(2));
    assert_eq!(cpu.cycles(), 2);
    // Starting on a breakpoint moves on.
    assert_eq!(cpu.run(1000, &[2]), RunStatus::Breakpoint(2));
    assert_eq!(cpu.cycles(), 6);
    assert_eq!(cpu.d(), 2);
    // A tight loop just consumes the budget.
    assert_eq!(cpu.run(1_000_000, &[]), RunStatus::Budget);
    assert_eq!(cpu.cycles(), 1_000_006);
    assert_eq!(cpu.run(0, &[0]), RunStatus::Budget);
    // The scratch bitset is cleared again.
    assert_eq!(cpu.run(10, &[]), RunStatus::Budget);
    assert_eq!(cpu.run(10, &[9999]), RunStatus::Budget);
}

#[test]
fn screen_generation_tracks_screen_writes() {
    let mut cpu = cpu_with("@SCREEN\nM=-1\n@100\nM=1\n");
    assert!(cpu.take_screen_dirty(), "loading clears the screen");
    assert!(!cpu.take_screen_dirty());
    cpu.step().unwrap();
    assert!(!cpu.take_screen_dirty());
    cpu.step().unwrap();
    assert!(cpu.take_screen_dirty());
    assert_eq!(cpu.screen()[0], 0xFFFF);
    cpu.run(2, &[]);
    assert!(!cpu.take_screen_dirty());
    cpu.set_ram(24575, 1);
    assert!(cpu.take_screen_dirty());
    assert_eq!(cpu.screen()[8191], 1);
}

#[test]
fn load_keeps_ram_but_clears_screen_and_registers() {
    let mut cpu = cpu_with("@7\nD=A\n");
    cpu.run(2, &[]);
    cpu.set_ram(5, 7);
    cpu.set_ram(16384, 9);
    cpu.set_key(65);
    cpu.load_rom(&[1]);
    assert_eq!((cpu.a(), cpu.d(), cpu.pc()), (0, 0, 0));
    assert_eq!((cpu.ram(5), cpu.ram(16384), cpu.key()), (7, 0, 65));
    assert_eq!(cpu.rom_len(), 1);
    assert_eq!(cpu.rom(1), crate::isa::EMPTY_WORD);
    assert_eq!(cpu.ram_slice().len(), 24577);
    assert_eq!(cpu.rom_slice().len(), 32768);
    assert!(!cpu.set_ram(24577, 1));
    assert_eq!(cpu.ram(40000), 0);
}

#[test]
fn every_comp_matches_the_reference_alu() {
    // Independent formulation of the documented computations.
    let reference = |m: &str, a: u16, d: u16, mm: u16| -> u16 {
        let y = if m.contains('M') { mm } else { a };
        let x = |s: &str| if s == "D" { d } else { y };
        match m {
            "0" => 0,
            "1" => 1,
            "-1" => 0xFFFF,
            "!D" | "!A" | "!M" => !x(&m[1..]),
            "-D" | "-A" | "-M" => x(&m[1..]).wrapping_neg(),
            "D" | "A" | "M" => x(m),
            _ => {
                let (l, op, r) = (&m[0..1], &m[1..2], &m[2..3]);
                let lv = x(l);
                let rv = if r == "1" { 1 } else { x(r) };
                match op {
                    "+" => lv.wrapping_add(rv),
                    "-" => lv.wrapping_sub(rv),
                    "&" => lv & rv,
                    _ => lv | rv,
                }
            }
        }
    };
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as u16
    };
    for &(m, c) in &crate::isa::COMPS {
        for _ in 0..50 {
            let (a, d, mm) = (next() % 24577, next(), next());
            let mut cpu = Cpu::new();
            cpu.load_rom(&[crate::isa::encode_c(c, 2, 0)]);
            cpu.set_a(a);
            cpu.set_d(d);
            cpu.set_ram(a, mm);
            cpu.step().unwrap();
            assert_eq!(cpu.d(), reference(m, a, d, mm), "{m} a={a} d={d} m={mm}");
        }
    }
}

#[test]
#[ignore = "speed measurement; run with --release --ignored"]
fn speed() {
    let mut cpu = cpu_with(include_str!("../../tests/fixtures/Speed.asm"));
    let start = std::time::Instant::now();
    let n = 500_000_000;
    assert_eq!(cpu.run(n, &[]), RunStatus::Budget);
    let mips = n as f64 / start.elapsed().as_secs_f64() / 1e6;
    eprintln!("{mips:.0} million instructions per second");
    assert!(mips > 50.0, "{mips:.0} MIPS");
}
