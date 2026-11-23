//! Golden tests for original hand-written VM programs.
//!
//! Expected RAM values and 8192-word screen checksums (FNV-1a 64-bit over
//! little-endian screen words, plus non-zero word count and sampled words)
//! were captured directly from the official Java VM Emulator. These tests
//! are completely self-contained and require neither Java nor course files.

use n2t_vm::{Breakpoints, Program, Vm, parse};

fn fnv1a_screen(screen: &[i16]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &w in screen {
        for b in (w as u16).to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    h
}

#[test]
fn golden_segments_and_arith() {
    let parsed = vec![parse(
        "SegArith",
        r#"// Original test: all segments and arithmetic/logical operations
push constant 10
pop local 0
push constant 21
push constant 22
pop argument 2
pop argument 1
push constant 36
pop this 6
push constant 42
push constant 45
pop that 5
pop that 2
push constant 510
pop temp 6
push local 0
push that 5
add
push argument 1
sub
push this 6
push this 6
add
sub
push temp 6
add
push constant 3030
pop pointer 0
push constant 3040
pop pointer 1
push constant 77
pop static 3
push pointer 0
push pointer 1
add
push static 3
sub
push constant 100
push constant 100
eq
push constant 5
push constant 9
lt
push constant 9
push constant 5
gt
and
or
neg
not
"#,
    )];
    for f in &parsed {
        assert!(!f.has_errors(), "parse errors: {:?}", f.diagnostics);
    }
    let (prog, diags) = Program::link(parsed);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "link errors: {diags:?}"
    );
    let mut vm = Vm::new(prog);
    vm.set_var("sp", None, 256).unwrap();
    vm.set_var("local", None, 300).unwrap();
    vm.set_var("argument", None, 400).unwrap();
    vm.set_var("this", None, 3000).unwrap();
    vm.set_var("that", None, 3010).unwrap();
    let _ = vm.run(48, 0, &Breakpoints::new());
    let expected_ram: &[(u16, i16)] = &[
        (0, 259),
        (3, 3030),
        (4, 3040),
        (11, 510),
        (19, 77),
        (256, 472),
        (257, 5993),
        (258, 1),
        (300, 10),
        (401, 21),
        (402, 22),
        (3006, 36),
        (3012, 42),
        (3015, 45),
    ];
    for &(addr, val) in expected_ram {
        assert_eq!(vm.ram(addr), val, "RAM[{addr}] mismatch");
    }
}

#[test]
fn golden_multifile_recursion() {
    let parsed = vec![
        parse(
            "Sys",
            r#"// Original test: multi-file calls, recursion, and per-file statics
function Sys.init 0
push constant 7
call Calc.fib 1
pop temp 0
push constant 6
call Calc.fact 1
pop temp 1
push constant 111
call Store.setA 1
pop temp 2
push constant 222
call Store.setB 1
pop temp 2
call Store.sum 0
pop temp 2
call Calc.getCalls 0
pop temp 3
label HALT
goto HALT
"#,
        ),
        parse(
            "Calc",
            r#"function Calc.fib 0
push static 0
push constant 1
add
pop static 0
push argument 0
push constant 2
lt
if-goto BASE
push argument 0
push constant 1
sub
call Calc.fib 1
push argument 0
push constant 2
sub
call Calc.fib 1
add
return
label BASE
push argument 0
return

function Calc.fact 0
push argument 0
push constant 2
lt
if-goto ONE
push argument 0
push argument 0
push constant 1
sub
call Calc.fact 1
call Calc.mul 2
return
label ONE
push constant 1
return

function Calc.mul 2
push constant 0
pop local 0
push argument 1
pop local 1
label MLOOP
push local 1
push constant 0
eq
if-goto MDONE
push local 0
push argument 0
add
pop local 0
push local 1
push constant 1
sub
pop local 1
goto MLOOP
label MDONE
push local 0
return

function Calc.getCalls 0
push static 0
return
"#,
        ),
        parse(
            "Store",
            r#"function Store.setA 0
push argument 0
pop static 0
push constant 0
return

function Store.setB 0
push argument 0
pop static 2
push constant 0
return

function Store.sum 0
push static 0
push static 2
add
return
"#,
        ),
    ];
    for f in &parsed {
        assert!(!f.has_errors(), "parse errors: {:?}", f.diagnostics);
    }
    let (prog, diags) = Program::link(parsed);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "link errors: {diags:?}"
    );
    let mut vm = Vm::new(prog);
    let _ = vm.run(1500, 0, &Breakpoints::new());
    let expected_ram: &[(u16, i16)] = &[(5, 13), (6, 0), (7, 0), (8, 0)];
    for &(addr, val) in expected_ram {
        assert_eq!(vm.ram(addr), val, "RAM[{addr}] mismatch");
    }
}

#[test]
fn golden_os_math_memory_array() {
    let parsed = vec![parse(
        "Main",
        r#"// Original test: Math, Memory allocator, and Array
function Main.main 0
push constant 123
neg
call Math.abs 1
pop static 0
push constant 32767
push constant 1
add
call Math.abs 1
pop static 1
push constant 32767
push constant 2
call Math.multiply 2
pop static 2
push constant 19
neg
push constant 4
call Math.divide 2
pop static 3
push constant 1000
call Math.sqrt 1
pop static 4
push constant 32767
call Math.sqrt 1
pop static 5
push constant 50
neg
push constant 20
call Math.min 2
pop static 6
push constant 50
neg
push constant 20
call Math.max 2
pop static 7
// Memory allocator: split + reuse + forward coalesce
push constant 5
call Memory.alloc 1
pop static 8
push constant 1
call Array.new 1
pop static 9
push static 8
call Memory.deAlloc 1
pop temp 0
push constant 3
call Memory.alloc 1
pop static 10
push constant 6
call Memory.alloc 1
pop static 11
push constant 1
call Memory.alloc 1
pop static 12
push static 11
call Memory.deAlloc 1
pop temp 0
push constant 3
call Memory.alloc 1
pop static 13
push constant 1
call Memory.alloc 1
pop static 14
push static 9
call Array.dispose 1
pop temp 0
push constant 8000
push constant 9876
call Memory.poke 2
pop temp 0
push constant 8000
call Memory.peek 1
pop static 15
push constant 0
return
"#,
    )];
    for f in &parsed {
        assert!(!f.has_errors(), "parse errors: {:?}", f.diagnostics);
    }
    let (prog, diags) = Program::link(parsed);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "link errors: {diags:?}"
    );
    let mut vm = Vm::new(prog);
    let _ = vm.run(500, 0, &Breakpoints::new());
    let expected_ram: &[(u16, i16)] = &[
        (16, 123),
        (17, -32768),
        (18, -2),
        (19, -4),
        (20, 31),
        (21, 181),
        (22, -50),
        (23, 20),
        (24, 2050),
        (25, 2057),
        (26, 2050),
        (27, 2060),
        (28, 2068),
        (29, 2060),
        (30, 2065),
        (31, 9876),
        (2048, 0),
        (2049, 2055),
        (2050, 0),
        (2051, 0),
        (2052, 0),
        (2053, 0),
        (2054, 0),
        (2055, 1),
        (2056, 2058),
        (2057, 0),
        (2058, 0),
        (2059, 2063),
        (2060, 0),
        (2061, 0),
        (2062, 0),
        (2063, 0),
        (2064, 2066),
        (2065, 0),
        (2066, 0),
        (2067, 2069),
        (2068, 0),
        (2069, 14313),
        (2070, 16384),
        (2071, 0),
        (2072, 0),
        (2073, 0),
        (2074, 0),
        (2075, 0),
    ];
    for &(addr, val) in expected_ram {
        assert_eq!(vm.ram(addr), val, "RAM[{addr}] mismatch");
    }
}

#[test]
fn golden_os_string() {
    let parsed = vec![parse(
        "Main",
        r#"// Original test: String operations and heap layout
function Main.main 1
push constant 0
call String.new 1
pop static 0
push constant 8
call String.new 1
pop static 1
push static 1
push constant 32767
push constant 1
add
call String.setInt 2
pop temp 0
push static 1
call String.intValue 1
pop static 2
push static 1
call String.length 1
pop static 3
push static 1
push constant 0
call String.charAt 2
pop static 4
push static 1
call String.eraseLastChar 1
pop temp 0
push static 1
push constant 2
push constant 57
call String.setCharAt 3
pop temp 0
push static 1
call String.intValue 1
pop static 5
push constant 6
call String.new 1
pop local 0
push local 0
push constant 45
call String.appendChar 2
push constant 55
call String.appendChar 2
push constant 56
call String.appendChar 2
push constant 88
call String.appendChar 2
pop temp 0
push local 0
call String.intValue 1
pop static 6
call String.backSpace 0
pop static 7
call String.doubleQuote 0
pop static 8
call String.newLine 0
pop static 9
push local 0
call String.dispose 1
pop temp 0
push constant 0
return
"#,
    )];
    for f in &parsed {
        assert!(!f.has_errors(), "parse errors: {:?}", f.diagnostics);
    }
    let (prog, diags) = Program::link(parsed);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "link errors: {diags:?}"
    );
    let mut vm = Vm::new(prog);
    let _ = vm.run(500, 0, &Breakpoints::new());
    let expected_ram: &[(u16, i16)] = &[
        (16, 2050),
        (17, 2054),
        (18, -32768),
        (19, 6),
        (20, 45),
        (21, -3976),
        (22, -78),
        (23, 129),
        (24, 34),
        (25, 128),
        (2048, 0),
        (2049, 2052),
        (2050, 0),
        (2051, 0),
        (2052, 0),
        (2053, 2064),
        (2054, 8),
        (2055, 5),
        (2056, 45),
        (2057, 51),
        (2058, 57),
        (2059, 55),
        (2060, 54),
        (2061, 56),
        (2062, 0),
        (2063, 0),
        (2064, 14318),
        (2065, 16384),
        (2066, 6),
        (2067, 4),
        (2068, 45),
        (2069, 55),
        (2070, 56),
        (2071, 88),
    ];
    for &(addr, val) in expected_ram {
        assert_eq!(vm.ram(addr), val, "RAM[{addr}] mismatch");
    }
}

#[test]
fn golden_os_screen() {
    let parsed = vec![parse(
        "Main",
        r#"// Original test: Screen pixels, 8-octant lines, rectangles, circles
function Main.main 0
call Screen.clearScreen 0
pop temp 0
push constant 1
call Screen.setColor 1
pop temp 0
push constant 7
push constant 9
call Screen.drawPixel 2
pop temp 0
push constant 10
push constant 10
push constant 41
push constant 22
call Screen.drawLine 4
pop temp 0
push constant 91
push constant 22
push constant 60
push constant 10
call Screen.drawLine 4
pop temp 0
push constant 10
push constant 45
push constant 41
push constant 33
call Screen.drawLine 4
pop temp 0
push constant 120
push constant 10
push constant 132
push constant 51
call Screen.drawLine 4
pop temp 0
push constant 150
push constant 51
push constant 162
push constant 10
call Screen.drawLine 4
pop temp 0
push constant 180
push constant 10
push constant 210
push constant 40
call Screen.drawLine 4
pop temp 0
push constant 25
push constant 80
push constant 125
push constant 140
call Screen.drawRectangle 4
pop temp 0
push constant 0
call Screen.setColor 1
pop temp 0
push constant 40
push constant 95
push constant 110
push constant 125
call Screen.drawRectangle 4
pop temp 0
push constant 1
call Screen.setColor 1
pop temp 0
push constant 250
push constant 100
push constant 0
call Screen.drawCircle 3
pop temp 0
push constant 280
push constant 100
push constant 5
call Screen.drawCircle 3
pop temp 0
push constant 360
push constant 120
push constant 35
call Screen.drawCircle 3
pop temp 0
push constant 0
return
"#,
    )];
    for f in &parsed {
        assert!(!f.has_errors(), "parse errors: {:?}", f.diagnostics);
    }
    let (prog, diags) = Program::link(parsed);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "link errors: {diags:?}"
    );
    let mut vm = Vm::new(prog);
    let _ = vm.run(500, 0, &Breakpoints::new());
    let expected_ram: &[(u16, i16)] = &[(0, 261)];
    for &(addr, val) in expected_ram {
        assert_eq!(vm.ram(addr), val, "RAM[{addr}] mismatch");
    }
    assert_eq!(vm.screen_slice().iter().filter(|&&w| w != 0).count(), 812);
    assert_eq!(
        fnv1a_screen(vm.screen_slice()),
        0x2E7F12C05A2C6A8C_u64,
        "8192-word screen hash mismatch vs Java"
    );
    for &(idx, val) in &[
        (288, 128),
        (320, 3072),
        (323, 12288),
        (327, 256),
        (330, 4),
        (331, 16),
        (352, 12288),
        (355, -16384),
        (359, 256),
        (362, 4),
        (363, 32),
        (384, -16384),
    ] {
        assert_eq!(vm.screen_slice()[idx], val, "screen[{idx}] mismatch");
    }
}

#[test]
fn golden_os_output() {
    let parsed = vec![parse(
        "Main",
        r#"// Original test: Output all printable ASCII, fallback, printInt, wrap, backSpace
function Main.main 0
push constant 32
call Output.printChar 1
pop temp 0
push constant 33
call Output.printChar 1
pop temp 0
push constant 34
call Output.printChar 1
pop temp 0
push constant 35
call Output.printChar 1
pop temp 0
push constant 36
call Output.printChar 1
pop temp 0
push constant 37
call Output.printChar 1
pop temp 0
push constant 38
call Output.printChar 1
pop temp 0
push constant 39
call Output.printChar 1
pop temp 0
push constant 40
call Output.printChar 1
pop temp 0
push constant 41
call Output.printChar 1
pop temp 0
push constant 42
call Output.printChar 1
pop temp 0
push constant 43
call Output.printChar 1
pop temp 0
push constant 44
call Output.printChar 1
pop temp 0
push constant 45
call Output.printChar 1
pop temp 0
push constant 46
call Output.printChar 1
pop temp 0
push constant 47
call Output.printChar 1
pop temp 0
push constant 48
call Output.printChar 1
pop temp 0
push constant 49
call Output.printChar 1
pop temp 0
push constant 50
call Output.printChar 1
pop temp 0
push constant 51
call Output.printChar 1
pop temp 0
push constant 52
call Output.printChar 1
pop temp 0
push constant 53
call Output.printChar 1
pop temp 0
push constant 54
call Output.printChar 1
pop temp 0
push constant 55
call Output.printChar 1
pop temp 0
push constant 56
call Output.printChar 1
pop temp 0
push constant 57
call Output.printChar 1
pop temp 0
push constant 58
call Output.printChar 1
pop temp 0
push constant 59
call Output.printChar 1
pop temp 0
push constant 60
call Output.printChar 1
pop temp 0
push constant 61
call Output.printChar 1
pop temp 0
push constant 62
call Output.printChar 1
pop temp 0
push constant 63
call Output.printChar 1
pop temp 0
push constant 64
call Output.printChar 1
pop temp 0
push constant 65
call Output.printChar 1
pop temp 0
push constant 66
call Output.printChar 1
pop temp 0
push constant 67
call Output.printChar 1
pop temp 0
push constant 68
call Output.printChar 1
pop temp 0
push constant 69
call Output.printChar 1
pop temp 0
push constant 70
call Output.printChar 1
pop temp 0
push constant 71
call Output.printChar 1
pop temp 0
push constant 72
call Output.printChar 1
pop temp 0
push constant 73
call Output.printChar 1
pop temp 0
push constant 74
call Output.printChar 1
pop temp 0
push constant 75
call Output.printChar 1
pop temp 0
push constant 76
call Output.printChar 1
pop temp 0
push constant 77
call Output.printChar 1
pop temp 0
push constant 78
call Output.printChar 1
pop temp 0
push constant 79
call Output.printChar 1
pop temp 0
push constant 80
call Output.printChar 1
pop temp 0
push constant 81
call Output.printChar 1
pop temp 0
push constant 82
call Output.printChar 1
pop temp 0
push constant 83
call Output.printChar 1
pop temp 0
push constant 84
call Output.printChar 1
pop temp 0
push constant 85
call Output.printChar 1
pop temp 0
push constant 86
call Output.printChar 1
pop temp 0
push constant 87
call Output.printChar 1
pop temp 0
push constant 88
call Output.printChar 1
pop temp 0
push constant 89
call Output.printChar 1
pop temp 0
push constant 90
call Output.printChar 1
pop temp 0
push constant 91
call Output.printChar 1
pop temp 0
push constant 92
call Output.printChar 1
pop temp 0
push constant 93
call Output.printChar 1
pop temp 0
push constant 94
call Output.printChar 1
pop temp 0
push constant 95
call Output.printChar 1
pop temp 0
push constant 96
call Output.printChar 1
pop temp 0
push constant 97
call Output.printChar 1
pop temp 0
push constant 98
call Output.printChar 1
pop temp 0
push constant 99
call Output.printChar 1
pop temp 0
push constant 100
call Output.printChar 1
pop temp 0
push constant 101
call Output.printChar 1
pop temp 0
push constant 102
call Output.printChar 1
pop temp 0
push constant 103
call Output.printChar 1
pop temp 0
push constant 104
call Output.printChar 1
pop temp 0
push constant 105
call Output.printChar 1
pop temp 0
push constant 106
call Output.printChar 1
pop temp 0
push constant 107
call Output.printChar 1
pop temp 0
push constant 108
call Output.printChar 1
pop temp 0
push constant 109
call Output.printChar 1
pop temp 0
push constant 110
call Output.printChar 1
pop temp 0
push constant 111
call Output.printChar 1
pop temp 0
push constant 112
call Output.printChar 1
pop temp 0
push constant 113
call Output.printChar 1
pop temp 0
push constant 114
call Output.printChar 1
pop temp 0
push constant 115
call Output.printChar 1
pop temp 0
push constant 116
call Output.printChar 1
pop temp 0
push constant 117
call Output.printChar 1
pop temp 0
push constant 118
call Output.printChar 1
pop temp 0
push constant 119
call Output.printChar 1
pop temp 0
push constant 120
call Output.printChar 1
pop temp 0
push constant 121
call Output.printChar 1
pop temp 0
push constant 122
call Output.printChar 1
pop temp 0
push constant 123
call Output.printChar 1
pop temp 0
push constant 124
call Output.printChar 1
pop temp 0
push constant 125
call Output.printChar 1
pop temp 0
push constant 126
call Output.printChar 1
pop temp 0
call Output.println 0
pop temp 0
push constant 32767
push constant 1
add
call Output.printInt 1
pop temp 0
push constant 0
call Output.printChar 1
pop temp 0
push constant 129
call Output.printChar 1
pop temp 0
push constant 33
call Output.printChar 1
pop temp 0
push constant 22
push constant 63
call Output.moveCursor 2
pop temp 0
push constant 90
call Output.printChar 1
pop temp 0
push constant 89
call Output.printChar 1
pop temp 0
push constant 1
push constant 0
call Output.moveCursor 2
pop temp 0
call Output.backSpace 0
pop temp 0
push constant 88
call Output.printChar 1
pop temp 0
push constant 0
return
"#,
    )];
    for f in &parsed {
        assert!(!f.has_errors(), "parse errors: {:?}", f.diagnostics);
    }
    let (prog, diags) = Program::link(parsed);
    assert!(
        diags.iter().all(|d| !d.is_error()),
        "link errors: {diags:?}"
    );
    let mut vm = Vm::new(prog);
    let _ = vm.run(1000, 0, &Breakpoints::new());
    let expected_ram: &[(u16, i16)] = &[(0, 261)];
    for &(addr, val) in expected_ram {
        assert_eq!(vm.ram(addr), val, "RAM[{addr}] mismatch");
    }
    assert_eq!(vm.screen_slice().iter().filter(|&&w| w != 0).count(), 441);
    assert_eq!(
        fnv1a_screen(vm.screen_slice()),
        0xAD67BC14E7EDA2BB_u64,
        "8192-word screen hash mismatch vs Java"
    );
    for &(idx, val) in &[
        (32, 3123),
        (33, 54),
        (34, 12),
        (35, 3084),
        (36, 1560),
        (40, 3084),
        (41, 7710),
        (42, 16144),
        (43, 16156),
        (44, 7710),
        (47, 7680),
        (48, 3102),
    ] {
        assert_eq!(vm.screen_slice()[idx], val, "screen[{idx}] mismatch");
    }
}
