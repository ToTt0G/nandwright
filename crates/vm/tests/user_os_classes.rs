//! Programs with their own OS classes (`Memory.init`, `Memory.alloc`, …) and
//! no `Sys.vm`, so the built-in `Sys.init` boots them. The expected RAM was
//! captured from the Java VM Emulator (GUI, built-in OS) running each program
//! for the given number of steps. The programs are kept here instead of as
//! `.vm` files because files named like OS classes are not allowed in the
//! repository.

use n2t_vm::{Breakpoints, Program, RunStatus, Vm, parse};

fn run(files: &[(&str, &str)], steps: u64) -> (Vm, RunStatus) {
    let parsed: Vec<_> = files.iter().map(|(stem, src)| parse(stem, src)).collect();
    let (program, diags) = Program::link(parsed);
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:?}");
    let mut vm = Vm::new(program);
    let status = vm.run(steps, 0, &Breakpoints::new());
    (vm, status)
}

fn check(vm: &Vm, expected: &[(u16, i16)]) {
    for &(address, value) in expected {
        assert_eq!(vm.ram(address), value, "RAM[{address}]");
    }
}

/// Each init adds one to RAM[7009] and stores the count in its own cell, so
/// RAM[7001..=7005] show the order the built-in `Sys.init` called them in.
fn os_init(class: &str, cell: u16) -> String {
    format!(
        "function {class}.init 0\npush constant 7000\npop pointer 1\npush that 9\npush constant 1\nadd\npop that 9\npush that 9\npop that {cell}\npush constant 0\nreturn\n"
    )
}

fn os_inits() -> Vec<(&'static str, String)> {
    ["Memory", "Math", "Screen", "Output", "Keyboard"]
        .into_iter()
        .zip(1..)
        .map(|(class, cell)| (class, os_init(class, cell)))
        .collect()
}

#[test]
fn the_built_in_sys_init_runs_the_programs_os_inits_in_javas_order() {
    let inits = os_inits();
    let mut files: Vec<(&str, &str)> = inits.iter().map(|(c, s)| (*c, s.as_str())).collect();
    files.push(("Main", "// Runs after the OS init functions.\nfunction Main.main 0\npush constant 7000\npop pointer 1\npush constant 99\npop that 8\npush constant 0\nreturn\n"));
    let (vm, status) = run(&files, 100);
    assert_eq!(status, RunStatus::Halted);
    // Java leaves the heap to the program's own Memory.init: RAM[2048] stays 0.
    check(
        &vm,
        &[
            (0, 261),
            (1, 261),
            (2, 256),
            (3, 0),
            (4, 0),
            (2048, 0),
            (2049, 0),
            (2050, 0),
            (2051, 0),
            (7000, 0),
            (7001, 1),
            (7002, 2),
            (7003, 3),
            (7004, 4),
            (7005, 5),
            (7006, 0),
            (7007, 0),
            (7008, 99),
            (7009, 5),
        ],
    );
}

#[test]
fn the_built_in_sys_init_takes_javas_steps_for_the_programs_os_inits() {
    let inits = os_inits();
    let mut files: Vec<(&str, &str)> = inits.iter().map(|(c, s)| (*c, s.as_str())).collect();
    files.push(("Main", "// Never reached in 40 steps.\nfunction Main.main 0\npush constant 7000\npop pointer 1\npush constant 99\npop that 8\nlabel STAY\ngoto STAY\n"));
    // 40 steps stop inside Output.init.
    let (vm, status) = run(&files, 40);
    assert_eq!(status, RunStatus::Budget);
    check(
        &vm,
        &[
            (0, 267),
            (1, 266),
            (2, 261),
            (3, 0),
            (4, 7000),
            (261, -1),
            (262, 261),
            (263, 256),
            (264, 0),
            (265, 0),
            (2048, 0),
            (2049, 0),
            (2050, 0),
            (2051, 0),
            (7000, 0),
            (7001, 1),
            (7002, 2),
            (7003, 3),
            (7004, 0),
            (7005, 0),
            (7006, 0),
            (7007, 0),
            (7008, 0),
            (7009, 3),
        ],
    );
}

#[test]
fn built_in_array_and_string_functions_use_the_programs_own_memory() {
    // A bump allocator from 3000 that logs every call at 7100: alloc logs the
    // size, deAlloc logs minus the address.
    let memory = "// A bump allocator from 3000 that logs every call at 7100:\n// alloc logs the size, deAlloc logs minus the address.\nfunction Memory.init 0\npush constant 3000\npop static 0\npush constant 0\npop static 1\npush constant 0\nreturn\nfunction Memory.alloc 0\npush constant 7100\npush static 1\nadd\npop pointer 1\npush argument 0\npop that 0\npush static 1\npush constant 1\nadd\npop static 1\npush static 0\npush static 0\npush argument 0\nadd\npop static 0\nreturn\nfunction Memory.deAlloc 0\npush constant 7100\npush static 1\nadd\npop pointer 1\npush argument 0\nneg\npop that 0\npush static 1\npush constant 1\nadd\npop static 1\npush constant 0\nreturn\n";
    // Array.new, String.new, String.appendChar and String.length, then the two
    // dispose calls; then a loop counts at RAM[7009] how many steps are left.
    let main = "// Built-in Array and String functions with the program's own Memory.\nfunction Main.main 1\npush constant 3\ncall Array.new 1\npop local 0\npush constant 7000\npop pointer 1\npush local 0\npop that 0\npush constant 4\ncall String.new 1\npop temp 0\npush constant 7000\npop pointer 1\npush temp 0\npop that 1\npush temp 0\npush constant 65\ncall String.appendChar 2\npop temp 1\npush temp 0\ncall String.length 1\npop temp 2\npush constant 7000\npop pointer 1\npush temp 2\npop that 2\npush local 0\ncall Array.dispose 1\npop temp 2\npush temp 0\ncall String.dispose 1\npop temp 2\nlabel COUNT\npush constant 7000\npop pointer 1\npush that 9\npush constant 1\nadd\npop that 9\ngoto COUNT\n";
    let (vm, status) = run(&[("Memory", memory), ("Main", main)], 300);
    assert_eq!(status, RunStatus::Budget);
    check(
        &vm,
        &[
            (0, 267),
            (1, 266),
            (2, 261),
            (3, 0),
            (4, 7000),
            (2048, 0),
            (2049, 0),
            (2050, 0),
            (2051, 0),
            (3000, 0),
            (3001, 0),
            (3002, 0),
            (3003, 4),
            (3004, 1),
            (3005, 65),
            (3006, 0),
            (3007, 0),
            (3008, 0),
            (3009, 0),
            (3010, 0),
            (3011, 0),
            (7000, 3000),
            (7001, 3003),
            (7002, 1),
            (7003, 0),
            (7004, 0),
            (7005, 0),
            (7006, 0),
            (7007, 0),
            (7008, 0),
            (7009, 28),
            (7100, 3),
            (7101, 6),
            (7102, -3000),
            (7103, -3003),
            (7104, 0),
            (7105, 0),
        ],
    );
}
