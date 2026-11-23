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
fn check_accepts_the_fixture_chips() {
    let (code, _, stderr) = n2t(&["check", samples().join("chips").to_str().unwrap()]);
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

/// Review finding A3/A4: an unimplemented skeleton is the normal state of a
/// course project. It is a hint, not an error, and it still runs.
#[test]
fn an_unimplemented_chip_is_not_an_error() {
    let (code, _, stderr) = n2t(&[
        "check",
        samples().join("runtime/Stub.hdl").to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.contains("hint[hdl-empty-implementation]"),
        "{stderr}"
    );
    assert!(stderr.contains(" 0 errors, 0 warnings"), "{stderr}");
}

#[test]
fn test_runs_the_fixture_chips() {
    let dir = scratch_copy(&samples().join("chips"), "chips");
    let (code, stdout, stderr) = n2t(&["test", dir.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("6 passed, 0 failed"), "{stdout}");
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

/// The official simulator loads an unimplemented chip, simulates it with its
/// outputs at zero and reports the first compare line that differs. The
/// `.out` file ends with that line.
#[test]
fn an_unimplemented_chip_fails_on_comparison_like_the_official_simulator() {
    let dir = scratch_copy(&samples().join("runtime"), "stub");
    let (code, stdout, stderr) = n2t(&["test", dir.join("Stub.tst").to_str().unwrap()]);
    let out = fs::read_to_string(dir.join("Stub.out")).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stderr.contains("comparison failure at line 3"), "{stderr}");
    assert_eq!(
        out,
        "|   a   |   b   |  out  |\n|   0   |   0   |   0   |\n|   0   |   1   |   0   |\n"
    );
}

#[test]
fn test_json_reports_outcomes_for_tools() {
    let dir = scratch_copy(&samples().join("runtime"), "json");
    let (_, stdout, _) = n2t(&[
        "test",
        "--json",
        dir.join("Stub.tst").to_str().unwrap(),
        dir.join("Unchecked.tst").to_str().unwrap(),
    ]);
    let _ = fs::remove_dir_all(&dir);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 2, "{stdout}");
    assert_eq!(records[0]["outcome"], "failed");
    assert_eq!(records[0]["dialect"], "hardware");
    assert_eq!(records[0]["mismatch"]["line"], 3);
    assert_eq!(records[0]["mismatch"]["script_line"], 7);
    assert_eq!(records[1]["outcome"], "unchecked");
}

#[test]
fn eval_prints_pins_including_internal_ones() {
    let chip = samples().join("chips/Majority.hdl");
    let (code, stdout, stderr) = n2t(&["eval", chip.to_str().unwrap(), "a=1", "c=1"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("OUT out     1  (1)"), "{stdout}");
    assert!(stdout.contains("INT ac      1  (1)"), "{stdout}");
    assert!(stdout.contains("INT ab      0  (0)"), "{stdout}");
}

#[test]
fn eval_steps_a_clocked_chip() {
    let chip = samples().join("chips/Accumulator.hdl");
    let (code, stdout, stderr) = n2t(&[
        "eval",
        chip.to_str().unwrap(),
        "in=5",
        "add=1",
        "--cycles",
        "3",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("0000000000001111  (15)"), "{stdout}");
    assert!(stdout.contains("time 3"), "{stdout}");
}

/// A script that waits for a key is run by pressing that key for it.
#[test]
fn test_presses_the_key_a_script_waits_for() {
    let dir = scratch_copy(&samples().join("chips"), "keys");
    let tst = dir.join("KeyLatch.tst");
    let (code, stdout, stderr) = n2t(&["test", tst.to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("[pressed 'K' for the script]"), "{stdout}");

    // Without it the script waits until the command limit, as it would wait
    // for a person in the official simulator.
    let (code, stdout, stderr) = n2t(&[
        "test",
        "--no-keys",
        "--max-commands",
        "5000",
        tst.to_str().unwrap(),
    ]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stdout.contains("still running after 5000 commands"),
        "{stdout}"
    );
    assert!(stderr.contains("Hold down the 'K' key"), "{stderr}");
}

/// The course's own `tools/builtInChips` files, which a student may copy
/// next to their chips, are not UTF-8 and name polymorphic built-ins.
#[test]
fn check_reads_windows_1252_and_polymorphic_course_builtins() {
    let dir = std::env::temp_dir().join(format!("n2t-cli-course-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

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

    let hdl = dir.join("Nor.hdl");
    fs::write(
        &hdl,
        "CHIP Nor{\nIN a,b;\n  OUT out;\nPARTS:\nOr(a=a,b=b,out=w);\n      Not(in=w,out=out);\n}",
    )
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
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        content,
        "CHIP Nor {\n    IN a, b;\n    OUT out;\n    PARTS:\n    Or(a=a, b=b, out=w);\n    Not(in=w, out=out);\n}\n"
    );
}

/// Review finding A1: the formatter used to move `,` and `;` onto the end of
/// a `//` comment, silently commenting them out.
#[test]
fn fmt_never_comments_out_punctuation() {
    let dir = std::env::temp_dir().join(format!("n2t-cli-fmt-comment-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let hdl = dir.join("Pair.hdl");
    fs::write(
        &hdl,
        "CHIP Pair {\n    IN a // first\n    , b;\n    OUT out;\n    PARTS:\n    Nand(a=a // left\n    , b=b, out=out);\n}\n",
    )
    .unwrap();
    let (code, stdout, stderr) = n2t(&["fmt", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let (code, _, stderr) = n2t(&["check", dir.to_str().unwrap()]);
    let content = fs::read_to_string(&hdl).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        code, 0,
        "the formatted chip no longer checks:\n{content}\n{stderr}"
    );
    assert!(content.contains("IN a // first\n"), "{content}");
    assert!(content.contains(", b;"), "{content}");
}

/// Review finding A2: formatting a Windows-1252 file must not re-encode it.
#[test]
fn fmt_keeps_the_encoding_of_windows_1252_files() {
    let dir = std::env::temp_dir().join(format!("n2t-cli-fmt-cp1252-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let hdl = dir.join("Dash.hdl");
    let mut bytes = b"// a ".to_vec();
    bytes.push(0x96);
    bytes.extend_from_slice(b" b\nCHIP Dash {\nIN a;\nOUT out;\nPARTS:\nOr(a=a,b=a,out=out);\n}\n");
    fs::write(&hdl, &bytes).unwrap();
    let (code, stdout, stderr) = n2t(&["fmt", dir.to_str().unwrap()]);
    let after = fs::read(&hdl).unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(after.starts_with(b"// a \x96 b\n"), "{after:?}");
    assert!(
        std::str::from_utf8(&after).is_err(),
        "the file was re-encoded"
    );
    assert!(
        after.ends_with(b"    Or(a=a, b=a, out=out);\n}\n"),
        "{after:?}"
    );
}

/// Review finding A4: a chip built on a neighbour that is not written yet
/// loads, and fails on a comparison, as in the official simulator.
#[test]
fn a_chip_using_an_unimplemented_neighbour_still_runs() {
    let dir = scratch_copy(&samples().join("runtime"), "neighbour");
    let (code, stdout, stderr) = n2t(&["test", dir.join("UsesStub.tst").to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stderr.contains("comparison failure at line 3"), "{stderr}");
    assert!(!stderr.contains("cannot load"), "{stderr}");
}

/// Review finding A5: a compare file that does not exist is reported.
#[test]
fn a_missing_compare_file_is_an_error() {
    let dir = scratch_copy(&samples().join("runtime"), "nocmp");
    let (code, stdout, stderr) = n2t(&["test", dir.join("MissingCompare.tst").to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stderr.contains("the compare file `NoSuchFile.cmp` does not exist"),
        "{stderr}"
    );
}

/// Review finding A3: scripts for the CPU and VM emulators are not errors,
/// and they run.
#[test]
fn cpu_and_vm_emulator_scripts_check_clean_and_pass() {
    let dir = scratch_copy(&samples().join("programs"), "dialects");
    let (code, _, stderr) = n2t(&["check", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains(" 0 errors, 0 warnings"), "{stderr}");
    let (code, stdout, stderr) = n2t(&["test", "--json", dir.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(code, 0, "{stdout}{stderr}");
    let dialects: Vec<String> = stdout
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .map(|r| {
            format!(
                "{}:{}",
                r["dialect"].as_str().unwrap(),
                r["outcome"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(dialects, ["vm:passed", "cpu:passed"]);
}

/// Review finding A7: messages never show the Windows `\\?\` path form, and
/// an unsupported file type is not called a syntax error.
#[test]
fn messages_use_plain_paths() {
    let dir = scratch_copy(&samples().join("runtime"), "paths");
    let (_, stdout, stderr) = n2t(&["test", dir.join("Stub.tst").to_str().unwrap()]);
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !stdout.contains(r"\\?\") && !stderr.contains(r"\\?\"),
        "{stdout}{stderr}"
    );
}

/// Review finding A7: a file the formatter does not handle is called
/// unsupported, not syntactically wrong.
#[test]
fn fmt_says_when_a_file_type_is_not_supported() {
    let asm = samples().join("programs/Countdown.asm");
    let (code, stdout, stderr) = n2t(&["fmt", "--check", asm.to_str().unwrap()]);
    assert_eq!(code, 2, "{stdout}{stderr}");
    assert!(
        stderr.contains("only `.hdl` and `.tst` files can be formatted"),
        "{stdout}{stderr}"
    );
    assert!(!stderr.contains("syntax errors"), "{stderr}");
}
