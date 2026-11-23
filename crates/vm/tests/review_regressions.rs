//! Regression tests for defects found in review: each one failed (panicked,
//! hung, grew without bound or behaved differently from the Java VM Emulator)
//! before its fix.

use n2t_vm::{Breakpoints, Program, RunStatus, Segment, Vm, parse};

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
