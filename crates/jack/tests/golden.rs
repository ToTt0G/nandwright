//! Golden tests: the original programs in `tests/fixtures/*` compile to exactly
//! the `.vm` files the official JackCompiler produced for them.

use std::fs;
use std::path::{Path, PathBuf};

use n2t_jack::{compile, compile_program};

fn fixture_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && p.file_name().unwrap() != "errors")
        .collect();
    dirs.sort();
    dirs
}

fn jack_files(dir: &Path) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "jack"))
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn fixtures_match_official_output() {
    let dirs = fixture_dirs();
    assert!(dirs.len() >= 5);
    for dir in dirs {
        let files = jack_files(&dir);
        let pairs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let out = compile_program(&pairs);
        for f in &out.files {
            let errors: Vec<_> = f.compiled.diagnostics.iter().filter(|d| d.is_error()).collect();
            assert!(errors.is_empty(), "{}/{}: {errors:?}", dir.display(), f.file_stem);
            let golden = fs::read_to_string(dir.join(format!("{}.vm", f.file_stem)))
                .unwrap()
                .replace("\r\n", "\n");
            let vm = f.compiled.vm.as_deref().unwrap();
            assert_eq!(vm, golden, "{}/{}.vm", dir.display(), f.file_stem);
            assert_eq!(f.compiled.line_map.len(), vm.lines().count());
        }
        // Compiling each file on its own gives the same code.
        for (stem, src) in &files {
            let c = compile(stem, src);
            let golden = fs::read_to_string(dir.join(format!("{stem}.vm"))).unwrap();
            assert_eq!(c.vm.as_deref(), Some(golden.replace("\r\n", "\n").as_str()));
        }
    }
}

#[test]
fn line_map_points_at_the_source_line() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Counter");
    let src = fs::read_to_string(dir.join("Counter.jack")).unwrap();
    let c = compile("Counter", &src);
    let vm = c.vm.unwrap();
    let lines: Vec<&str> = src.lines().collect();
    for (cmd, line) in vm.lines().zip(&c.line_map) {
        let jack = lines[*line as usize];
        if let Some(name) = cmd.strip_prefix("function Counter.") {
            let name = name.split(' ').next().unwrap();
            assert!(jack.contains(name), "{cmd} -> {jack}");
        }
        if cmd == "call Memory.deAlloc 1" {
            assert!(jack.contains("Memory.deAlloc"));
        }
        if cmd.starts_with("label WHILE_EXP") {
            assert!(jack.contains("while"));
        }
    }
}

#[test]
fn cross_class_errors_fixture() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/errors/CrossClass");
    let files = jack_files(&dir);
    let pairs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let out = compile_program(&pairs);
    let main = out.files.iter().find(|f| f.file_stem == "Main").unwrap();
    let shape = out.files.iter().find(|f| f.file_stem == "Shape").unwrap();
    assert!(shape.compiled.vm.is_some());
    assert!(main.compiled.vm.is_none());
    // Lines the official compiler reports for this file.
    let official: Vec<u32> = main
        .compiled
        .diagnostics
        .iter()
        .zip(&main.compiled.official_lines)
        .filter(|(d, _)| d.is_error())
        .map(|(_, l)| *l)
        .collect();
    assert_eq!(official, vec![6, 7, 7, 8, 9, 10, 11]);
    // The OS and unknown-class calls are warnings only.
    let warnings: Vec<_> = main
        .compiled
        .diagnostics
        .iter()
        .filter(|d| d.severity == n2t_core::Severity::Warning)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(warnings, ["jack-argument-count", "jack-unknown-subroutine"]);
    // With the lenient option the code is still produced, as the official
    // compiler does.
    let lenient = n2t_jack::compile_program_with(
        &pairs,
        &n2t_jack::Options {
            keep_vm_on_call_errors: true,
        },
    );
    assert!(lenient.files.iter().all(|f| f.compiled.vm.is_some()));
}
