//! Replays original probes captured by conformance/vm/capture_java_probes.py.
//! Every captured screen word is checked, including zeroes.

use std::fs;
use std::path::Path;
use std::str::FromStr;

use n2t_vm::{Breakpoints, Program, RunStatus, Vm, VmError, parse};

fn number<T: FromStr>(s: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    s.parse().unwrap()
}

fn replay(name: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/java_probes")
        .join(name);
    let mut files = Vec::new();
    for path in fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()) {
        if path.extension().is_some_and(|ext| ext == "vm") {
            let parsed = parse(
                path.file_stem().unwrap().to_str().unwrap(),
                &fs::read_to_string(&path).unwrap(),
            );
            assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
            files.push(parsed);
        }
    }
    files.sort_by(|a, b| a.file_stem.cmp(&b.file_stem));
    let (program, diagnostics) = Program::link(files);
    assert!(diagnostics.iter().all(|d| !d.is_error()), "{diagnostics:?}");
    let mut vm = Vm::new(program);
    let bp = Breakpoints::new();
    let mut budget = 500;
    let mut keys = Vec::new();
    let mut screen = None;
    let mut faults = false;
    for line in fs::read_to_string(dir.join("probe.txt")).unwrap().lines() {
        let parts: Vec<_> = line.split('#').next().unwrap().split_whitespace().collect();
        match parts.as_slice() {
            ["steps", count] => budget = number(count),
            ["keys" | "codes", codes @ ..] => {
                keys = codes.iter().map(|code| number::<u16>(code)).collect();
            }
            ["screen"] => screen = Some(vec![0i16; 8192]),
            ["set", variable, value] => {
                let (name, index) = variable
                    .split_once('[')
                    .map_or((*variable, None), |(n, i)| {
                        (n, Some(number(i.strip_suffix(']').unwrap())))
                    });
                vm.set_var(name, index, number(value)).unwrap();
            }
            ["fault"] => faults = true,
            [] | ["fast" | "single"] | ["ram", _, _] => {}
            _ => panic!("Unexpected directive {line}"),
        }
    }
    let run = |vm: &mut Vm| vm.run(budget - vm.steps(), 0, &bp);
    let mut status = run(&mut vm);
    for key in keys {
        assert_eq!(status, RunStatus::WaitingForKey, "{name}: before key {key}");
        vm.set_key(key);
        assert_eq!(
            run(&mut vm),
            RunStatus::WaitingForKey,
            "{name}: held key {key}"
        );
        vm.set_key(0);
        status = run(&mut vm);
    }
    let expected = fs::read_to_string(dir.join("expected.txt")).unwrap();
    let mut java_halted = false;
    let mut java_error = false;
    let mut java_status = "";
    for line in expected.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["ram", addr, value] => {
                assert_eq!(
                    vm.ram(number(addr)),
                    number::<i16>(value),
                    "{name}: RAM[{addr}]"
                );
            }
            ["screen", index, value] => {
                screen.as_mut().unwrap()[number::<usize>(index)] = number(value);
            }
            ["java-dialog", ..] if line.contains("Main.main finished execution") => {
                java_halted = true;
            }
            ["java-dialog", ..] if line.contains("Program Halted:") => java_error = true,
            ["java", ..] => java_status = line.strip_prefix("java ").unwrap(),
            ["java-dialog", ..] => {}
            _ => panic!("Unexpected golden line {line}"),
        }
    }
    if let Some(screen) = screen {
        let diffs: Vec<_> = vm
            .screen_slice()
            .iter()
            .zip(&screen)
            .enumerate()
            .filter(|(_, (actual, expected))| actual != expected)
            .collect();
        assert!(
            diffs.is_empty(),
            "{name}: {} screen words differ (index, (Rust, Java)): {:?}",
            diffs.len(),
            &diffs[..diffs.len().min(12)]
        );
    }
    if faults {
        assert_eq!(status, RunStatus::Budget, "{name}");
        let actual = vm.run(1, 0, &bp);
        let matches_java = match java_status {
            s if s.starts_with("Stack overflow in ") => {
                matches!(actual, RunStatus::Error(VmError::StackOverflow { .. }))
            }
            s if s.starts_with("Out of segment space in ") => {
                matches!(actual, RunStatus::Error(VmError::OutOfSegmentSpace { .. }))
            }
            "A built-in function tried to access memory outside the Heap or Screen range" => {
                matches!(actual, RunStatus::Error(VmError::MemoryOutOfRange { .. }))
            }
            _ => false,
        };
        assert!(matches_java, "{name}: Java {java_status}, Rust {actual:?}");
    } else if java_halted {
        assert_eq!(status, RunStatus::Halted, "{name}");
    } else if java_error {
        assert!(matches!(status, RunStatus::Error(_)), "{name}: {status:?}");
    }
}

macro_rules! java_probes {
    ($($name:ident),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                replay(stringify!($name));
            }
        )+
    };
}

java_probes!(
    kb_gui_mapping,
    kb_last_cell,
    kb_readchar_specials,
    kb_readint_heap,
    kb_readint_parse,
    kb_readline_backspace_bounds,
    kb_readline_edit,
    kb_readline_long,
    negative_circle_bottom,
    negative_circle_end_aligned,
    negative_circle_keyboard,
    negative_circle_large,
    negative_circle_min,
    negative_circle_start_aligned,
    negative_circle_top,
    negative_circle_word_boundary,
    screen_edge_details,
    screen_edges,
);
