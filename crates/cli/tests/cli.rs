//! End-to-end checks of the `n2t` binary against the repository's samples.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .unwrap()
}

fn n2t(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_n2t"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Copies a directory tree into a fresh temporary folder (scripts write `.out` files).
fn scratch_copy(from: &Path, tag: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap().map(Result::unwrap) {
            let path = entry.path();
            if path.is_dir() {
                copy(&path, &to.join(entry.file_name()));
            } else {
                fs::copy(&path, to.join(entry.file_name())).unwrap();
            }
        }
    }
    let target = std::env::temp_dir().join(format!("n2t-cli-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&target);
    copy(from, &target);
    target
}

#[test]
fn check_accepts_the_projects() {
    let (code, _, stderr) = n2t(&["check", samples().join("projects").to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains(" 0 errors, 0 warnings"), "{stderr}");
}

#[test]
fn check_reports_errors_with_exit_code_1() {
    let (code, _, stderr) = n2t(&[
        "check",
        samples()
            .join("errors/hdl/WidthMismatch.hdl")
            .to_str()
            .unwrap(),
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("error[hdl-width-mismatch]"), "{stderr}");
    assert!(stderr.contains("WidthMismatch.hdl:6:9"), "{stderr}");
}

#[test]
fn test_runs_the_projects() {
    let dir = scratch_copy(&samples().join("projects"), "projects");
    let (code, stdout, stderr) = n2t(&["test", dir.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("33 passed, 0 failed"), "{stdout}");
}

#[test]
fn test_reports_a_comparison_failure() {
    let dir = scratch_copy(&samples().join("runtime"), "runtime");
    let (code, stdout, stderr) = n2t(&["test", dir.join("Mismatch.tst").to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert!(stdout.contains("FAIL"), "{stdout}");
    assert!(
        stderr.contains("comparison failure at line 2, column 11"),
        "{stderr}"
    );
}

#[test]
fn eval_prints_pins() {
    let alu = samples().join("projects/02/ALU.hdl");
    // x + y with x = 2, y = 3.
    let (code, stdout, stderr) = n2t(&["eval", alu.to_str().unwrap(), "x=2", "y=3", "f=1"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("OUT out") && stdout.contains("0000000000000101  (5)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("INT xPlusY") && stdout.contains("0000000000000101  (5)"),
        "expected internal pins in eval output:\n{stdout}"
    );
}

#[test]
fn test_accepts_scalar_register_index_zero_like_official_computer_add() {
    let dir = scratch_copy(&samples().join("projects/05"), "scalar-zero");
    let tst = dir.join("ComputerAdd.tst");
    let original = fs::read_to_string(&tst).unwrap();
    // Match official nand2tetris/projects/05/ComputerAdd.tst spelling.
    let official_spelling = original
        .replace("ARegister[]", "ARegister[0]")
        .replace("DRegister[]", "DRegister[0]");
    fs::write(&tst, &official_spelling).unwrap();

    let (code, stdout, stderr) = n2t(&["test", tst.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("PASS"), "{stdout}");
}

#[test]
fn test_keys_flag_drives_interactive_keyboard_loops() {
    let dir = scratch_copy(&samples().join("projects/05"), "keys-loop");
    let tst = dir.join("KeyTest.tst");
    let cmp = dir.join("KeyTest.cmp");
    fs::write(
        &tst,
        "\
load Memory.hdl,
output-file KeyTest.out,
compare-to KeyTest.cmp,
output-list address%D1.5.1 out%D1.3.1;

set address 24576,
echo \"Hold K\",
while out <> 75 {
    eval,
}
clear-echo,
output;

echo \"Hold Y\",
while out <> 89 {
    tick, tock;
}
clear-echo,
output;
",
    )
    .unwrap();
    fs::write(
        &cmp,
        "\
|address| out |
| 24576 |  75 |
| 24576 |  89 |
",
    )
    .unwrap();

    let (code, stdout, stderr) = n2t(&["test", "--keys", "K,Y", tst.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("PASS"), "{stdout}");
}

#[test]
fn check_reads_windows_1252_and_polymorphic_course_builtins() {
    let dir = std::env::temp_dir().join(format!("n2t-cli-course-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // Mirrors `tools/builtInChips/ALU.hdl` (Windows-1252 0x96 en-dash in doc comment)
    // and `tools/builtInChips/And16.hdl` (`BUILTIN And;` for 16-bit And16).
    let alu = dir.join("ALU.hdl");
    let mut alu_bytes = b"/** Computes x+y, x-y, y".to_vec();
    alu_bytes.push(0x96);
    alu_bytes.extend_from_slice(
        b"x */\nCHIP ALU {\n    IN x[16], y[16], zx, nx, zy, ny, f, no;\n    OUT out[16], zr, ng;\n    BUILTIN ALU;\n}\n",
    );
    fs::write(&alu, alu_bytes).unwrap();

    let and16 = dir.join("And16.hdl");
    fs::write(
        &and16,
        "CHIP And16 {\n    IN a[16], b[16];\n    OUT out[16];\n    BUILTIN And;\n}\n",
    )
    .unwrap();

    let (code, _, stderr) = n2t(&["check", dir.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("0 errors, 0 warnings"), "{stderr}");
}

#[test]
fn missing_path_exits_with_code_2() {
    let (code, _, stderr) = n2t(&["check", "definitely/not/here"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("does not exist"), "{stderr}");
}

#[test]
fn fmt_formats_and_checks_hdl_and_tst_files() {
    let dir = std::env::temp_dir().join(format!("n2t-cli-fmt-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let hdl = dir.join("And.hdl");
    fs::write(&hdl, "CHIP And{IN a,b;OUT out;PARTS: }")
        .unwrap();

    // `--check` exits 1 when unformatted.
    let (code, stdout, stderr) = n2t(&["fmt", "--check", dir.to_str().unwrap()]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("would format"), "{stdout}");

    // Format in place exits 0.
    let (code, stdout, stderr) = n2t(&["fmt", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("formatted"), "{stdout}");

    // `--check` now passes with exit 0.
    let (code, stdout, stderr) = n2t(&["fmt", "--check", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}{stderr}");

    let content = fs::read_to_string(&hdl).unwrap();
    assert_eq!(
        content,
        "CHIP And {\n    IN a, b;\n    OUT out;\n\n    PARTS: }\n"
    );
    let _ = fs::remove_dir_all(&dir);
}
