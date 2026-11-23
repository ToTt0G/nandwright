//! Test scripts in the three dialects, and `.out` files compared with their
//! `.cmp` file.

mod common;

use common::{Harness, pos_of, slice, url};
use lsp_types as lsp;
use n2t_core::host::MemoryHost;

const DIR: &str = "c:/Users/me/My Chips";

fn file_url(name: &str) -> String {
    format!("file:///c:/Users/me/My%20Chips/{name}")
}

fn doc_url(name: &str) -> lsp::Url {
    url(&format!("file:///c%3A/Users/me/My%20Chips/{name}"))
}

const CPU_SCRIPT: &str = "\
load Countdown.asm,
output-file Countdown.out,
compare-to Countdown.cmp,
output-list RAM[100]%D2.6.2 RAM[101]%D2.6.2;

set PC 0,
repeat 60 {
    ticktock;
}
output;
";

fn cpu_folder() -> Harness {
    let host = MemoryHost::new()
        .with(&format!("{DIR}/Countdown.asm"), "@10\nD=A\n")
        .with(
            &format!("{DIR}/Countdown.cmp"),
            "|RAM[100]|RAM[101]|\n|    10  |     9  |\n",
        )
        .with(&format!("{DIR}/Countdown.out"), "")
        .with(&format!("{DIR}/Spare.hack"), "0000000000000001\n")
        .with(&format!("{DIR}/Doubler.vm"), "push constant 2\n");
    Harness::new(host)
}

#[test]
fn cpu_scripts_get_cpu_commands_variables_and_hover() {
    let mut h = cpu_folder();
    let uri = doc_url("Countdown.tst");
    let diagnostics = h.open(&uri, CPU_SCRIPT);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let hover = h.hover(&uri, pos_of(CPU_SCRIPT, "RAM[101]", 0, 1)).unwrap();
    assert!(hover.contains("a word of data memory"), "{hover}");
    assert!(hover.contains("CPU Emulator: writable"), "{hover}");
    let hover = h.hover(&uri, pos_of(CPU_SCRIPT, "PC 0", 0, 0)).unwrap();
    assert!(hover.contains("the program counter"), "{hover}");
    let hover = h.hover(&uri, pos_of(CPU_SCRIPT, "ticktock", 0, 2)).unwrap();
    assert!(hover.contains("executes one instruction"), "{hover}");

    // Commands of this dialect only.
    let line = CPU_SCRIPT.lines().count() as u32;
    let mut text = format!("{CPU_SCRIPT}\n");
    h.change(&uri, &text);
    let commands = h.labels(&uri, lsp::Position::new(line, 0));
    assert!(commands.contains(&"ticktock".into()), "{commands:?}");
    for other in ["eval", "tick", "tock", "vmstep"] {
        assert!(!commands.contains(&other.into()), "{other}: {commands:?}");
    }
    text.push_str("set ");
    h.change(&uri, &text);
    let vars = h.labels(&uri, lsp::Position::new(line + 1, 4));
    for v in ["A", "D", "PC", "RAM[0]", "ROM[0]"] {
        assert!(vars.contains(&v.into()), "{v}: {vars:?}");
    }
    assert!(!vars.contains(&"time".into()), "read-only: {vars:?}");
    text.truncate(text.len() - 4);
    text.push_str("output-list ");
    h.change(&uri, &text);
    let vars = h.labels(&uri, lsp::Position::new(line + 1, 12));
    assert!(vars.contains(&"time".into()), "{vars:?}");

    // `load` offers programs.
    h.change(&uri, "load ");
    let files = h.labels(&uri, lsp::Position::new(0, 5));
    for f in ["Countdown.asm", "Spare.hack", "Doubler.vm"] {
        assert!(files.contains(&f.into()), "{f}: {files:?}");
    }
}

#[test]
fn go_to_the_files_a_script_names() {
    let mut h = cpu_folder();
    let uri = doc_url("Countdown.tst");
    h.open(&uri, CPU_SCRIPT);
    for (needle, file) in [
        ("Countdown.asm", "Countdown.asm"),
        ("Countdown.out", "Countdown.out"),
        ("Countdown.cmp", "Countdown.cmp"),
    ] {
        let def = h
            .definition(&uri, pos_of(CPU_SCRIPT, needle, 0, 3))
            .unwrap();
        assert_eq!(def.uri.as_str(), file_url(file));
        assert_eq!(def.range, lsp::Range::default());
    }
    // An open document wins over the file's URL.
    let asm = doc_url("Countdown.asm");
    h.open(&asm, "@10\nD=A\n");
    let def = h
        .definition(&uri, pos_of(CPU_SCRIPT, "Countdown.asm", 0, 3))
        .unwrap();
    assert_eq!(def.uri, asm);

    // A file that does not exist: nowhere to go.
    let text = CPU_SCRIPT.replace("Countdown.cmp", "Missing.cmp");
    h.change(&uri, &text);
    assert_eq!(h.definition(&uri, pos_of(&text, "Missing.cmp", 0, 2)), None);

    // `ROM32K load X.hack` in a hardware script.
    let script = "load Box.hdl,\nROM32K load Spare.hack,\neval;\n";
    let tst = doc_url("Box.tst");
    h.open(&tst, script);
    let def = h
        .definition(&tst, pos_of(script, "Spare.hack", 0, 2))
        .unwrap();
    assert_eq!(def.uri.as_str(), file_url("Spare.hack"));
}

#[test]
fn vm_scripts_get_vm_commands_and_variables() {
    let mut h = cpu_folder();
    let uri = doc_url("Doubler.tst");
    let script = "\
load Doubler.vm,
output-file Doubler.out,
output-list RAM[256]%D1.6.1 local[0]%D1.6.1 currentFunction%S1.20.1;
set sp 256,
vmstep;
output;
";
    let diagnostics = h.open(&uri, script);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let hover = h.hover(&uri, pos_of(script, "local[0]", 0, 2)).unwrap();
    assert!(hover.contains("the `local` segment"), "{hover}");
    assert!(hover.contains("VM Emulator"), "{hover}");
    let hover = h
        .hover(&uri, pos_of(script, "currentFunction", 0, 2))
        .unwrap();
    assert!(
        hover.contains("read-only") && hover.contains("text"),
        "{hover}"
    );
    let hover = h.hover(&uri, pos_of(script, "vmstep", 0, 1)).unwrap();
    assert!(hover.contains("one VM command"), "{hover}");

    let text = format!("{script}set ");
    h.change(&uri, &text);
    let vars = h.labels(&uri, lsp::Position::new(6, 4));
    for v in ["sp", "local", "local[0]", "temp[0]", "RAM[0]"] {
        assert!(vars.contains(&v.into()), "{v}: {vars:?}");
    }
    assert!(!vars.contains(&"currentFunction".into()), "{vars:?}");
    assert!(
        !vars.contains(&"temp".into()),
        "temp is indexed only: {vars:?}"
    );
    let text = format!("{script}\n");
    h.change(&uri, &text);
    let commands = h.labels(&uri, lsp::Position::new(6, 0));
    assert!(commands.contains(&"vmstep".into()), "{commands:?}");
    assert!(!commands.contains(&"ticktock".into()), "{commands:?}");
}

#[test]
fn undecided_scripts_offer_every_command() {
    let mut h = cpu_folder();
    let uri = doc_url("New.tst");
    h.open(&uri, "");
    let commands = h.labels(&uri, lsp::Position::new(0, 0));
    for c in ["load", "eval", "ticktock", "vmstep", "output-list"] {
        assert!(commands.contains(&c.into()), "{c}: {commands:?}");
    }
}

#[test]
fn hardware_script_internal_pins_and_unknown_pins() {
    let host = MemoryHost::new().with(
        "/work/Blend.hdl",
        "CHIP Blend {\n    IN a, b;\n    OUT out;\n    PARTS:\n    Or(a=a, b=b, out=w);\n    Not(in=w, out=out);\n}\n",
    );
    let mut h = Harness::new(host);
    let uri = url("file:///work/Blend.tst");
    let script = "load Blend.hdl,\noutput-file Blend.out,\noutput-list a b w out;\nset a 1,\neval,\noutput;\n";
    let diagnostics = h.open(&uri, script);
    assert!(
        diagnostics.is_empty(),
        "internal pins can be output: {diagnostics:?}"
    );
    let script = script.replace("set a 1", "set nope 1");
    let diagnostics = h.change(&uri, &script);
    let d = diagnostics
        .iter()
        .find(|d| slice(&script, d.range) == "nope")
        .expect("a diagnostic on the unknown pin");
    assert_eq!(d.severity, Some(lsp::DiagnosticSeverity::ERROR));
}

const GATE_CMP: &str = "\
|  a  |  b  | out |
|  0  |  0  |  1  |
|  0  |  1  |  *  |
|  1  |  0  |  0  |
";

#[test]
fn out_files_are_compared_with_their_cmp_file() {
    let host = MemoryHost::new().with(&format!("{DIR}/Gate.cmp"), GATE_CMP);
    let mut h = Harness::new(host);
    let uri = doc_url("Gate.out");

    // `*` matches any one character.
    let good =
        "|  a  |  b  | out |\n|  0  |  0  |  1  |\n|  0  |  1  |  0  |\n|  1  |  0  |  0  |\n";
    assert!(h.open(&uri, good).is_empty());
    let crlf = good.replace('\n', "\r\n");
    assert!(
        h.change(&uri, &crlf).is_empty(),
        "line endings do not matter"
    );

    let bad =
        "|  a  |  b  | out |\n|  0  |  0  |  0  |\n|  0  |  1  |  1  |\n|  1  |  1  |  0  |\n";
    let diagnostics = h.change(&uri, bad);
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert_eq!(diagnostics[0].range.start.line, 1);
    assert_eq!(slice(bad, diagnostics[0].range), "  0  ");
    assert_eq!(diagnostics[0].range.start.character, 13);
    assert!(
        diagnostics[0]
            .message
            .contains("line 2 differs from `Gate.cmp` (column `out`): expected `1`, found `0`"),
        "{}",
        diagnostics[0].message
    );
    // Only the first differing cell of a line is reported.
    assert_eq!(diagnostics[1].range.start.line, 3);
    assert_eq!(diagnostics[1].range.start.character, 7);
    assert!(diagnostics[1].message.contains("column `b`"));

    // A short output.
    let short = "|  a  |  b  | out |\n|  0  |  0  |  1  |\n";
    let diagnostics = h.change(&uri, short);
    assert_eq!(diagnostics.len(), 1);
    assert!(
        diagnostics[0].message.contains("stops after 2 lines"),
        "{diagnostics:?}"
    );
    assert_eq!(diagnostics[0].range.start.line, 1);

    // Editing the open compare file re-checks the output.
    let cmp = doc_url("Gate.cmp");
    h.change(&uri, good);
    let published = {
        h.open(&cmp, &GATE_CMP.replace("|  *  |", "|  1  |"));
        h.published()
    };
    let _ = published;
    let diagnostics = h.change(&uri, good);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].range.start.line, 2);

    // Without a compare file: nothing to say.
    let lone = doc_url("Lone.out");
    assert!(h.open(&lone, "| x |\n").is_empty());
}

#[test]
fn missing_compare_files_and_programs_are_reported() {
    let mut h = cpu_folder();
    let uri = doc_url("Countdown.tst");
    let text = CPU_SCRIPT
        .replace("Countdown.cmp", "Nope.cmp")
        .replace("load Countdown.asm", "load Gone.asm");
    let diagnostics = h.open(&uri, &text);
    let missing: Vec<(&str, Option<lsp::DiagnosticSeverity>)> = diagnostics
        .iter()
        .filter(|d| d.code == Some(lsp::NumberOrString::String("tst-missing-file".into())))
        .map(|d| (slice(&text, d.range), d.severity))
        .collect();
    assert_eq!(
        missing,
        [
            ("Gone.asm", Some(lsp::DiagnosticSeverity::ERROR)),
            ("Nope.cmp", Some(lsp::DiagnosticSeverity::WARNING)),
        ]
    );
    // Opening the compare file (an unsaved buffer) is enough.
    h.open(&doc_url("Nope.cmp"), "|x|\n");
    let diagnostics = h.change(&uri, &text);
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.message.contains("not in this script's folder"))
            .count(),
        1
    );
}
