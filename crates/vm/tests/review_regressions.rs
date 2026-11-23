//! Regression tests for defects found in review: each one failed (panicked,
//! hung, grew without bound or behaved differently from the Java VM Emulator)
//! before its fix.

use n2t_vm::{Breakpoints, Program, RunStatus, Segment, StepStatus, Vm, parse};

fn make_vm(files: &[(&str, &str)]) -> Vm {
    let parsed: Vec<_> = files.iter().map(|(stem, src)| parse(stem, src)).collect();
    let (program, diags) = Program::link(parsed);
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:?}");
    Vm::new(program)
}

#[test]
fn script_variable_index_overflow_is_an_error_not_a_panic() {
    let mut vm = make_vm(&[("Main", "function Main.main 0\npush constant 0\nreturn\n")]);
    for name in ["local", "argument", "this", "that", "temp", "RAM"] {
        for i in [i64::MAX, i64::MIN, -1] {
            assert!(vm.get_var(name, Some(i)).is_err(), "{name}[{i}]");
            assert!(vm.set_var(name, Some(i), 1).is_err(), "{name}[{i}]");
        }
    }
}

#[test]
fn static_slice_of_a_file_placed_past_ram_is_empty_not_a_panic() {
    // `First` claims statics 0..=32767, so `Second`'s statics start past RAM.
    let vm = make_vm(&[
        ("First", "function First.f 0\npush static 32767\nreturn\n"),
        ("Second", "function Second.g 0\npush static 0\nreturn\n"),
    ]);
    assert_eq!(vm.static_slice("Second"), Some(&[][..]));
    assert!(vm.static_slice("First").is_some());
}

#[test]
fn linking_a_forged_command_scope_cannot_overflow() {
    let first = parse("First", "push constant 1");
    let mut second = parse("Second", "push constant 2");
    second.commands[0].function = Some(u32::MAX);
    let (program, _) = Program::link(vec![first, second]);
    let mut vm = Vm::new(program);
    assert_eq!(vm.run(5, 0, &Breakpoints::new()), RunStatus::Halted);
    assert_eq!(vm.stack(), &[1, 2]);
}

#[test]
fn endless_recursion_with_flat_sp_keeps_the_shadow_call_stack_bounded() {
    // Main.f pops its own frame and calls itself again: SP never grows, so
    // the program runs forever without a stack overflow.
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 0\npush constant 0\npush constant 0\npush constant 0\npush constant 0\n\
         call Main.f 0\n\
         function Main.f 0\n\
         pop temp 0\npop temp 0\npop temp 0\npop temp 0\npop temp 0\n\
         call Main.f 0\n",
    )]);
    let status = vm.run(3_000_000, 0, &Breakpoints::new());
    assert_eq!(status, RunStatus::Budget);
    let stack = vm.call_stack();
    assert!(stack.len() <= 4096, "{} shadow frames", stack.len());
    let top = stack.last().expect("a frame");
    assert_eq!(top.function, "Main.f");
    assert_eq!(top.file.as_deref(), Some("Main"));
    let _ = vm.segment_slice(Segment::Static, 10);
}

#[test]
fn runtime_error_stops_until_explicit_reposition_or_reset() {
    let mut vm = make_vm(&[("Main", "pop temp 0\npush constant 7\n")]);
    let bp = Breakpoints::new();
    assert!(matches!(vm.run(20, 0, &bp), RunStatus::Error(_)));
    vm.set_var("sp", None, 257).unwrap();
    assert_eq!(vm.run(20, 0, &bp), RunStatus::Halted);
    assert_eq!(vm.step(0), StepStatus::Halted);
    assert_eq!(vm.pc(), 0);
    assert!(vm.set_var("line", None, (1i64 << 32) + 1).is_err());
    vm.set_var("line", None, 1).unwrap();
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.stack(), &[0, 7]);
    vm.reset();
    assert!(matches!(vm.step(0), StepStatus::Error(_)));
}

#[test]
fn suspended_call_rejects_reposition_and_reset_discards_its_frame() {
    let mut vm = make_vm(&[(
        "Main",
        "push constant 2\ncall Sys.wait 1\npop temp 0\ncall Keyboard.readChar 0\npop temp 1\n",
    )]);
    let bp = Breakpoints::new();
    assert_eq!(
        vm.run(20, u64::MAX - 1, &bp),
        RunStatus::Sleeping { until_ms: u64::MAX }
    );
    assert!(vm.set_var("line", None, 3).is_err());
    assert_eq!(vm.pc(), 1);
    vm.set_key(65);
    assert_eq!(vm.run(20, u64::MAX, &bp), RunStatus::WaitingForKey);
    assert!(vm.set_var("line", None, 0).is_err());
    vm.set_key(66);
    assert_eq!(vm.run(20, u64::MAX, &bp), RunStatus::WaitingForKey);
    vm.set_key(0);
    assert_eq!(vm.run(20, u64::MAX, &bp), RunStatus::Halted);
    assert_eq!(vm.ram(6), 65);

    vm.reset();
    assert_eq!(vm.run(20, 10, &bp), RunStatus::Sleeping { until_ms: 12 });
    vm.reset();
    assert_eq!(vm.sp(), 256);
    assert_eq!(vm.pc(), 0);
    assert_eq!(vm.run(20, 50, &bp), RunStatus::Sleeping { until_ms: 52 });
    assert_eq!(vm.run(20, 52, &bp), RunStatus::WaitingForKey);
    vm.set_key(90);
    assert_eq!(vm.step(52), StepStatus::WaitingForKey);
    vm.reset();
    assert_eq!(vm.key(), 0);
    assert_eq!(vm.run(20, 100, &bp), RunStatus::Sleeping { until_ms: 102 });
}

#[test]
fn return_through_a_script_modified_argument_marks_screen_dirty() {
    let mut vm = make_vm(&[("Main", "function Main.main 0\npush constant 9\nreturn\n")]);
    let bp = Breakpoints::new();
    assert_eq!(vm.run(3, 0, &bp), RunStatus::Budget);
    vm.set_var("argument", None, 16384).unwrap();
    vm.clear_screen_dirty();
    let generation = vm.screen_generation();
    assert_eq!(vm.step(0), StepStatus::Halted);
    assert_eq!(vm.screen_slice()[0], 9);
    assert!(vm.screen_dirty());
    assert_eq!(vm.screen_generation(), generation + 1);
}

#[test]
fn keyboard_line_budget_is_bounded_and_reset_discards_input() {
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\npush constant 0\ncall String.new 1\n\
         call Keyboard.readLine 1\npop temp 0\npush constant 0\nreturn\n",
    )]);
    let bp = Breakpoints::new();
    let key = |vm: &mut Vm, code| {
        vm.set_key(code);
        assert_eq!(vm.run(50, 0, &bp), RunStatus::WaitingForKey);
        vm.set_key(0);
        vm.run(50, 0, &bp)
    };
    assert_eq!(vm.run(50, 0, &bp), RunStatus::WaitingForKey);
    assert_eq!(vm.ram(2052), 14330, "input does not allocate while waiting");
    for _ in 0..15_000 {
        assert_eq!(key(&mut vm, 65), RunStatus::WaitingForKey);
    }
    assert!(matches!(key(&mut vm, 128), RunStatus::Error(_)));
    vm.reset();
    assert_eq!(vm.run(50, 0, &bp), RunStatus::WaitingForKey);
    assert_eq!(key(&mut vm, 65), RunStatus::WaitingForKey);
    vm.reset();
    assert_eq!(vm.run(50, 0, &bp), RunStatus::WaitingForKey);
    assert_eq!(key(&mut vm, 90), RunStatus::WaitingForKey);
    assert_eq!(key(&mut vm, 128), RunStatus::Halted);
    let ptr = vm.ram(5) as u16;
    assert_eq!(&vm.ram_slice()[ptr as usize..][..3], &[1, 1, 90]);
}
