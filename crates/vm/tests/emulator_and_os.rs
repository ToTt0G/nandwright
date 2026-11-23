//! Comprehensive unit tests for `Vm` execution, script variables, runtime errors,
//! debugger/UI inspection APIs, and every built-in Jack OS class (including
//! non-blocking `Sys.wait` and `Keyboard.read*`).

use n2t_vm::{Breakpoints, Program, RunStatus, Segment, StepStatus, Vm, VmError, parse};

fn make_vm(files: &[(&str, &str)]) -> Vm {
    let parsed = files.iter().map(|(stem, src)| parse(stem, src)).collect();
    let (prog, diags) = Program::link(parsed);
    let errs: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errs.is_empty(), "link errors: {errs:?}");
    Vm::new(prog)
}

#[test]
fn arithmetic_logic_and_wraparound() {
    let mut vm = make_vm(&[(
        "Arith",
        "\
push constant 32767
push constant 1
add
push constant 0
push constant 32767
sub
push constant 1
sub
neg
push constant 32767
push constant 1
add
push constant 32767
lt
push constant 32767
push constant 32767
push constant 1
add
gt
push constant 5
push constant 5
eq
push constant 5
push constant 6
eq
push constant 12
push constant 10
and
push constant 12
push constant 10
or
push constant 0
not
",
    )]);
    let bp = Breakpoints::new();
    assert_eq!(vm.run(100, 0, &bp), RunStatus::Halted);
    // Stack: [-32768, -32768, -1, -1, -1, 0, 8, 14, -1]
    assert_eq!(vm.stack(), &[-32768, -32768, -1, -1, -1, 0, 8, 14, -1]);
}

#[test]
fn script_variables_get_set_and_bounds() {
    let mut vm = make_vm(&[("A", "push constant 10\npush constant 20\nadd\n")]);

    // Case-sensitive scalars
    assert_eq!(vm.get_var("sp", None), Ok(256));
    assert!(vm.get_var("SP", None).is_err());
    assert!(vm.get_var("lcl", None).is_err());
    assert!(vm.get_var("arg", None).is_err());
    assert!(vm.get_var("pointer", Some(0)).is_err());
    assert!(vm.get_var("static", Some(0)).is_err());
    assert!(vm.get_var("stack", Some(0)).is_err());

    // Set pointers & segment cells
    assert_eq!(vm.set_var("local", None, 300), Ok(()));
    assert_eq!(vm.set_var("argument", None, 400), Ok(()));
    assert_eq!(vm.set_var("this", None, 3000), Ok(()));
    assert_eq!(vm.set_var("that", None, 3010), Ok(()));
    assert_eq!(vm.set_var("local", Some(2), 11), Ok(()));
    assert_eq!(vm.set_var("argument", Some(3), 22), Ok(()));
    assert_eq!(vm.set_var("this", Some(4), 33), Ok(()));
    assert_eq!(vm.set_var("that", Some(5), 44), Ok(()));
    assert_eq!(vm.set_var("temp", Some(0), 55), Ok(()));
    assert_eq!(vm.set_var("temp", Some(8), 66), Ok(())); // RAM[13]
    assert_eq!(vm.set_var("RAM", Some(24576), 65), Ok(()));

    assert_eq!(vm.ram(302), 11);
    assert_eq!(vm.ram(403), 22);
    assert_eq!(vm.ram(3004), 33);
    assert_eq!(vm.ram(3015), 44);
    assert_eq!(vm.ram(5), 55);
    assert_eq!(vm.ram(13), 66);
    assert_eq!(vm.key(), 65);

    // Out-of-range checks
    assert!(vm.set_var("sp", None, -1).is_err());
    assert!(vm.set_var("local", None, 24577).is_err());
    assert!(vm.get_var("RAM", Some(24577)).is_err());
    assert!(vm.get_var("RAM", Some(-1)).is_err());
    assert!(vm.get_var("temp", Some(-1)).is_err());
    assert!(vm.set_var("currentFunction", None, 0).is_err());
    assert_eq!(vm.get_var_text("currentFunction"), Ok(String::new()));

    // `set line` repositions PC
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.ram(256), 10);
    assert_eq!(vm.get_var_text("line"), Ok(".0".to_string()));
    assert_eq!(vm.set_var("line", None, 0), Ok(()));
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.ram(257), 10);
}

#[test]
fn runtime_error_conditions_match_java_bounds() {
    // Stack underflow on pop
    let mut vm = make_vm(&[("A", "pop temp 0\n")]);
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::StackUnderflow { .. })
    ));

    // Stack overflow at sp=2047
    let mut vm = make_vm(&[("A", "push constant 1\n")]);
    vm.set_var("sp", None, 2047).unwrap();
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::StackOverflow { .. })
    ));

    // sp=2046 push succeeds (new sp=2047)
    let mut vm = make_vm(&[("A", "push constant 42\n")]);
    vm.set_var("sp", None, 2046).unwrap();
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.sp(), 2047);
    assert_eq!(vm.ram(2046), 42);

    // temp 8 out of segment space
    let mut vm = make_vm(&[("A", "push temp 8\n")]);
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::OutOfSegmentSpace {
            segment: "temp",
            ..
        })
    ));

    // pointer 2 silently ignored (does not push or pop)
    let mut vm = make_vm(&[("A", "push constant 9\npop pointer 2\npush pointer 2\n")]);
    assert_eq!(vm.run(10, 0, &Breakpoints::new()), RunStatus::Halted);
    assert_eq!(vm.sp(), 257);
    assert_eq!(vm.ram(256), 9);

    // pop pointer 0 (this): allows <= 0 and 2048..=16383; rejects 1..=2047 and >= 16384
    let mut vm = make_vm(&[(
        "A",
        "push constant 1\nneg\npop pointer 0\npush constant 16384\npop pointer 0\n",
    )]);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.this(), -1);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::IllegalPointer {
            pointer: 0,
            value: 16384,
            ..
        })
    ));

    // pop pointer 1 (that): allows 0 and 2048..=24576; rejects < 0 and 1..=2047
    let mut vm = make_vm(&[(
        "A",
        "push constant 24576\npop pointer 1\npush constant 1\nneg\npop pointer 1\n",
    )]);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.that(), 24576);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::IllegalPointer {
            pointer: 1,
            value: -1,
            ..
        })
    ));

    // static 238 succeeds (RAM[254]), static 239 fails
    let mut vm = make_vm(&[(
        "A",
        "push constant 7\npop static 238\npush constant 8\npop static 239\n",
    )]);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.ram(254), 7);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::OutOfSegmentSpace {
            segment: "static",
            ..
        })
    ));

    // segment access bounds: local outside 256..=2047 fails; this at 16384 fails; that at 24576 succeeds
    let mut vm = make_vm(&[("A", "push local 0\n")]);
    vm.set_var("local", None, 2048).unwrap();
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::OutOfSegmentSpace {
            segment: "local",
            ..
        })
    ));

    let mut vm = make_vm(&[("A", "push this 0\n")]);
    vm.set_var("this", None, 16384).unwrap();
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::OutOfSegmentSpace {
            segment: "this",
            ..
        })
    ));

    let mut vm = make_vm(&[("A", "push that 0\n")]);
    vm.set_var("that", None, 24576).unwrap();
    vm.set_key(77);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.ram(256), 77);

    // Illegal return address
    let mut vm = make_vm(&[("A", "push constant 1\nreturn\n")]);
    vm.set_var("sp", None, 261).unwrap();
    vm.set_var("local", None, 261).unwrap();
    vm.set_var("argument", None, 256).unwrap();
    vm.set_ram(256, 999);
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert!(matches!(
        vm.step(0),
        StepStatus::Error(VmError::IllegalReturnAddress { address: 999, .. })
    ));
}

#[test]
fn breakpoints_and_call_stack_inspection() {
    let mut vm = make_vm(&[
        (
            "Sys",
            "function Sys.init 0\n\
             push constant 5\n\
             call Math2.double 1\n\
             pop temp 0\n\
             label END\n\
             goto END\n",
        ),
        (
            "Math2",
            "function Math2.double 1\n\
             push argument 0\n\
             push argument 0\n\
             add\n\
             pop local 0\n\
             push local 0\n\
             return\n",
        ),
    ]);

    let mut bp = Breakpoints::new();
    bp.insert("Math2", 5); // `push local 0`
    assert_eq!(
        vm.run(100, 0, &bp),
        RunStatus::Breakpoint {
            file: "Math2".to_string(),
            line: 5
        }
    );
    assert_eq!(vm.current_function(), "Math2.double");
    assert_eq!(vm.current_file(), Some("Math2"));
    assert_eq!(vm.current_line(), Some(5));

    let stack = vm.call_stack();
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0].function, "Sys.init");
    assert_eq!(stack[1].function, "Math2.double");
    assert_eq!(stack[1].arguments, vec![5]);
    assert_eq!(stack[1].locals, vec![10]);
    assert_eq!(
        vm.segment_base(Segment::Argument),
        Some(stack[1].argument_base)
    );
    assert_eq!(vm.segment_slice(Segment::Local, 1), &[10]);

    // Step past the breakpoint, then run to END
    assert_eq!(vm.step(0), StepStatus::Stepped);
    assert_eq!(vm.run(10, 0, &Breakpoints::new()), RunStatus::Budget);
    assert_eq!(vm.temp_slice()[0], 10);

    // Reset restores clean state
    vm.reset();
    assert_eq!(vm.sp(), 256);
    assert_eq!(vm.temp_slice()[0], 0);
    assert_eq!(vm.steps(), 0);
}

#[test]
fn non_blocking_sys_wait_suspends_and_resumes() {
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 50\n\
         call Sys.wait 1\n\
         pop temp 0\n\
         push constant 99\n\
         pop static 0\n\
         push constant 0\n\
         return\n",
    )]);
    let bp = Breakpoints::new();
    // At now_ms = 1000, Sys.wait(50) suspends until 1050.
    assert_eq!(
        vm.run(100, 1000, &bp),
        RunStatus::Sleeping { until_ms: 1050 }
    );
    assert_eq!(vm.ram(16), 0);
    // Still sleeping at 1049
    assert_eq!(vm.step(1049), StepStatus::Sleeping { until_ms: 1050 });
    assert_eq!(vm.ram(16), 0);
    // Resumes and finishes at 1050
    assert_eq!(vm.run(100, 1050, &bp), RunStatus::Halted);
    assert_eq!(vm.ram(16), 99);
}

#[test]
fn non_blocking_keyboard_read_char_line_and_int() {
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 1\n\
         call Keyboard.readChar 0\n\
         pop static 0\n\
         push constant 2\n\
         call String.new 1\n\
         pop local 0\n\
         push local 0\n\
         push constant 62\n\
         call String.appendChar 2\n\
         pop temp 0\n\
         push local 0\n\
         call Keyboard.readLine 1\n\
         pop static 1\n\
         push local 0\n\
         call Keyboard.readInt 1\n\
         pop static 2\n\
         push constant 0\n\
         return\n",
    )]);
    let bp = Breakpoints::new();

    // Helper: press key, tick, release key, tick.
    let type_keys = |vm: &mut Vm, keys: &[u16]| {
        for &k in keys {
            vm.set_key(k);
            let _ = vm.run(10, 0, &bp);
            vm.set_key(0);
            let _ = vm.run(10, 0, &bp);
        }
    };

    // 1. Wait inside readChar
    assert_eq!(vm.run(100, 0, &bp), RunStatus::WaitingForKey);
    let gen_before = vm.screen_generation();
    type_keys(&mut vm, &[65]); // 'A'
    assert_eq!(vm.ram(16), 65);
    assert!(vm.screen_generation() > gen_before);

    // 2. Now suspended inside readLine: type 'H', 'X', BS(129), 'I', Enter(128)
    assert_eq!(vm.run(100, 0, &bp), RunStatus::WaitingForKey);
    type_keys(&mut vm, &[72, 88, 129, 73, 128]);
    let line_ptr = vm.ram(17) as u16;
    assert!(line_ptr >= 2048);
    assert_eq!(vm.ram(line_ptr + 1), 2); // length == 2
    assert_eq!(vm.ram(line_ptr + 2), 72); // 'H'
    assert_eq!(vm.ram(line_ptr + 3), 73); // 'I'

    // 3. Now suspended inside readInt: type '-', '1', '2', '3', BS, '4', Enter -> -124
    assert_eq!(vm.run(100, 0, &bp), RunStatus::WaitingForKey);
    type_keys(&mut vm, &[45, 49, 50, 51, 129, 52, 128]);
    assert_eq!(vm.run(100, 0, &bp), RunStatus::Halted);
    assert_eq!(vm.ram(18), -124);
}

#[test]
fn screen_dirty_flag_and_generation_track_writes() {
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 10\n\
         push constant 20\n\
         call Screen.drawPixel 2\n\
         pop temp 0\n\
         push constant 0\n\
         return\n",
    )]);
    vm.clear_screen_dirty();
    assert!(!vm.screen_dirty());
    let g0 = vm.screen_generation();
    assert_eq!(vm.run(100, 0, &Breakpoints::new()), RunStatus::Halted);
    assert!(vm.screen_dirty());
    assert!(vm.screen_generation() > g0);
    // Pixel (10, 20) -> word 20*32 + 0 = 640, bit 10 = 1024
    assert_eq!(vm.screen_slice()[20 * 32], 1 << 10);
}

#[test]
fn builtin_os_errors_print_err_code_and_halt() {
    let cases = [
        (
            "push constant 5\npush constant 0\ncall Math.divide 2\npop temp 0\n",
            "Math.divide",
        ),
        (
            "push constant 1\nneg\ncall Math.sqrt 1\npop temp 0\n",
            "Math.sqrt",
        ),
        (
            "push constant 0\ncall Memory.alloc 1\npop temp 0\n",
            "Memory.alloc",
        ),
        (
            "push constant 0\ncall Array.new 1\npop temp 0\n",
            "Array.new",
        ),
        (
            "push constant 1\nneg\ncall String.new 1\npop temp 0\n",
            "String.new",
        ),
        (
            "push constant 512\npush constant 0\ncall Screen.drawPixel 2\npop temp 0\n",
            "Screen.drawPixel",
        ),
        (
            "push constant 23\npush constant 0\ncall Output.moveCursor 2\npop temp 0\n",
            "Output.moveCursor",
        ),
        (
            "push constant 1\nneg\ncall Sys.wait 1\npop temp 0\n",
            "Sys.wait",
        ),
    ];
    for (body, expected_fn) in cases {
        let src = format!("function Main.main 0\n{body}push constant 0\nreturn\n");
        let mut vm = make_vm(&[("Main", &src)]);
        let status = vm.run(100, 0, &Breakpoints::new());
        match status {
            RunStatus::Error(VmError::BuiltinError { function, .. }) => {
                assert_eq!(function, expected_fn);
            }
            other => panic!("expected BuiltinError({expected_fn}), got {other:?}"),
        }
        assert!(vm.is_halted());
        // ERR<code> was printed starting at row 0, col 0 -> row 0 scanlines non-zero
        assert!(vm.screen_slice()[32..320].iter().any(|&w| w != 0));
    }

    // Sys.error(42)
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\npush constant 42\ncall Sys.error 1\npop temp 0\npush constant 0\nreturn\n",
    )]);
    assert_eq!(
        vm.run(100, 0, &Breakpoints::new()),
        RunStatus::Error(VmError::SysError { code: 42 })
    );
    assert!(vm.screen_slice()[32..320].iter().any(|&w| w != 0));
}

#[test]
fn memory_allocator_split_and_forward_coalesce() {
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 5\ncall Memory.alloc 1\npop static 0\n\
         push constant 1\ncall Memory.alloc 1\npop static 1\n\
         push static 0\ncall Memory.deAlloc 1\npop temp 0\n\
         push constant 3\ncall Memory.alloc 1\npop static 2\n\
         push constant 6\ncall Memory.alloc 1\npop static 3\n\
         push constant 1\ncall Memory.alloc 1\npop static 4\n\
         push static 3\ncall Memory.deAlloc 1\npop temp 0\n\
         push constant 3\ncall Memory.alloc 1\npop static 5\n\
         push constant 1\ncall Memory.alloc 1\npop static 6\n\
         push constant 0\nreturn\n",
    )]);
    assert_eq!(vm.run(500, 0, &Breakpoints::new()), RunStatus::Halted);
    assert_eq!(vm.ram(16), 2050);
    assert_eq!(vm.ram(17), 2057);
    assert_eq!(vm.ram(18), 2050); // reused 5-word block without split (rem=2)
    assert_eq!(vm.ram(19), 2060);
    assert_eq!(vm.ram(20), 2068);
    assert_eq!(vm.ram(21), 2060); // split 6-word block into 3 + free(1) at 2063..2066
    assert_eq!(vm.ram(22), 2065); // took the 1-word split leftover!
    // Corrupt heap header intentionally: must return HeapOverflow, never panic
    let mut vm = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 2048\n\
         push constant 30000\n\
         call Memory.poke 2\n\
         pop temp 0\n\
         push constant 20000\n\
         call Memory.alloc 1\n\
         pop temp 0\n\
         push constant 0\n\
         return\n",
    )]);
    assert!(matches!(
        vm.run(100, 0, &Breakpoints::new()),
        RunStatus::Error(VmError::BuiltinError {
            function: "Memory.alloc",
            ..
        })
    ));
}

#[test]
fn screen_line_and_circle_edge_cases() {
    // Reversed line endpoints produce identical pixels; r=0 draws 1 pixel;
    // r=-2 draws (cx, cy-2) and (cx, cy+2) matching the Java VM Emulator.
    let mut vm1 = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 10\npush constant 20\npush constant 37\npush constant 31\n\
         call Screen.drawLine 4\npop temp 0\n\
         push constant 100\npush constant 100\npush constant 2\nneg\n\
         call Screen.drawCircle 3\npop temp 0\n\
         push constant 50\npush constant 50\npush constant 0\n\
         call Screen.drawCircle 3\npop temp 0\n\
         push constant 0\nreturn\n",
    )]);
    let mut vm2 = make_vm(&[(
        "Main",
        "function Main.main 0\n\
         push constant 37\npush constant 31\npush constant 10\npush constant 20\n\
         call Screen.drawLine 4\npop temp 0\n\
         push constant 100\npush constant 98\ncall Screen.drawPixel 2\npop temp 0\n\
         push constant 100\npush constant 102\ncall Screen.drawPixel 2\npop temp 0\n\
         push constant 50\npush constant 50\n\
         call Screen.drawPixel 2\npop temp 0\n\
         push constant 0\nreturn\n",
    )]);
    assert_eq!(vm1.run(100, 0, &Breakpoints::new()), RunStatus::Halted);
    assert_eq!(vm2.run(100, 0, &Breakpoints::new()), RunStatus::Halted);
    assert_eq!(vm1.screen_slice(), vm2.screen_slice());
}

#[test]
fn benchmark_tight_loop_executes_fast() {
    let mut vm = make_vm(&[(
        "Sys",
        "function Sys.init 2\n\
         push constant 0\n\
         pop local 0\n\
         label LOOP\n\
         push local 0\n\
         push constant 1\n\
         add\n\
         pop local 0\n\
         goto LOOP\n",
    )]);
    vm.set_var("local", None, 256).unwrap();
    let bp = Breakpoints::new();
    let steps = 200_000u64;
    assert_eq!(vm.run(steps, 0, &bp), RunStatus::Budget);
    assert_eq!(vm.steps(), steps);
}
