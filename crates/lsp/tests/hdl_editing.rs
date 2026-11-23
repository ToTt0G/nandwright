//! HDL editing aids: signature help in part calls, quick fixes, hover on
//! internal pins, the skeleton-shadows-built-in hint and quiet diagnostics.

mod common;

use common::{Harness, apply, edits_for, pos_of, slice, url};
use lsp_types as lsp;
use n2t_core::host::MemoryHost;

const PICK: &str = "\
CHIP Pick {
    IN x[16], y[16], s;
    OUT out[16];

    PARTS:
    Mux16(a=x, b=y, sel=s, out=out);
}
";

fn active_label(help: &lsp::SignatureHelp) -> String {
    let sig = &help.signatures[0];
    let Some(active) = help.active_parameter else {
        return String::new();
    };
    let params = sig.parameters.as_ref().unwrap();
    match &params[active as usize].label {
        lsp::ParameterLabel::LabelOffsets([a, b]) => {
            sig.label[*a as usize..*b as usize].to_string()
        }
        lsp::ParameterLabel::Simple(s) => s.clone(),
    }
}

#[test]
fn signature_help_in_part_calls_highlights_the_active_pin() {
    let mut h = Harness::empty();
    let uri = url("file:///c%3A/Users/me/My%20Chips/Pick.hdl");
    h.open(&uri, PICK);
    let help = h.signature_help(&uri, pos_of(PICK, "sel=s", 0, 4)).unwrap();
    assert_eq!(
        help.signatures[0].label,
        "Mux16(IN a[16], b[16], sel; OUT out[16])"
    );
    assert_eq!(active_label(&help), "sel");
    let help = h.signature_help(&uri, pos_of(PICK, "a=x", 0, 0)).unwrap();
    assert_eq!(active_label(&help), "a[16]");
    let help = h
        .signature_help(&uri, pos_of(PICK, "out=out", 0, 5))
        .unwrap();
    assert_eq!(active_label(&help), "out[16]");
    // Outside a part call: nothing.
    assert!(h.signature_help(&uri, pos_of(PICK, "IN x", 0, 0)).is_none());

    // Half-typed: right after `(` or `,`, the next unconnected pin.
    let half = "CHIP Pick {\n    IN x[16], y[16], s;\n    OUT out[16];\n    PARTS:\n    Mux16(";
    h.change(&uri, half);
    let help = h.signature_help(&uri, lsp::Position::new(4, 10)).unwrap();
    assert_eq!(active_label(&help), "a[16]");
    let half = format!("{half}a=x, ");
    h.change(&uri, &half);
    let help = h.signature_help(&uri, lsp::Position::new(4, 15)).unwrap();
    assert_eq!(active_label(&help), "b[16]");
    let half = format!("{half}sel=");
    h.change(&uri, &half);
    let help = h.signature_help(&uri, lsp::Position::new(4, 19)).unwrap();
    assert_eq!(active_label(&help), "sel");
}

#[test]
fn quick_fixes_suggest_names_for_unknown_chips_and_pins() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Gate3.hdl");
    let text = "\
CHIP Gate3 {
    IN a, b, c;
    OUT out;

    PARTS:
    Andd(a=a, b=b, out=ab);
    nand(a=ab, b=c, out=n);
    Or(a=n, bb=c, out=out);
}
";
    let diagnostics = h.open(&uri, text);
    let errors: Vec<&lsp::Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.severity == Some(lsp::DiagnosticSeverity::ERROR))
        .collect();
    // One error per mistake: an unknown chip does not also make its
    // connections unknown pins.
    assert_eq!(errors.len(), 3, "{errors:?}");

    let fix = |h: &mut Harness, needle: &str| -> Vec<(String, String)> {
        let at = pos_of(text, needle, 0, 1);
        h.code_actions(&uri, lsp::Range::new(at, at))
            .into_iter()
            .filter(|a| a.title.starts_with("Did you mean"))
            .map(|a| {
                let fixed = apply(text, &edits_for(a.edit.as_ref().unwrap(), &uri));
                (a.title, fixed)
            })
            .collect()
    };
    let andd = fix(&mut h, "Andd");
    assert_eq!(andd[0].0, "Did you mean `And`?");
    assert!(andd[0].1.contains("    And(a=a, b=b, out=ab);"));
    // A case-insensitive match is preferred to any near miss.
    let nand = fix(&mut h, "nand");
    assert_eq!(nand.len(), 1, "{nand:?}");
    assert_eq!(nand[0].0, "Did you mean `Nand`?");
    let bb = fix(&mut h, "bb=");
    assert_eq!(bb[0].0, "Did you mean `b`?");
    assert!(bb[0].1.contains("Or(a=n, b=c, out=out);"));
}

#[test]
fn quick_fix_adds_missing_connections_in_declared_order() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Choose.hdl");
    let text = "\
CHIP Choose {
    IN a, b, s;
    OUT out;

    PARTS:
    Mux(a=a, out=out);
    Not(in=s);
}
";
    h.open(&uri, text);
    let at = pos_of(text, "Mux", 0, 1);
    let actions = h.code_actions(&uri, lsp::Range::new(at, at));
    let add = actions
        .iter()
        .find(|a| a.title.contains("missing input connections"))
        .expect("an action for Mux");
    assert!(add.title.contains("(b, sel)"), "{}", add.title);
    let fixed = apply(text, &edits_for(add.edit.as_ref().unwrap(), &uri));
    assert!(
        fixed.contains("    Mux(a=a, out=out, b=, sel=);"),
        "{fixed}"
    );

    let at = pos_of(text, "Not", 0, 1);
    let actions = h.code_actions(&uri, lsp::Range::new(at, at));
    assert!(
        !actions
            .iter()
            .any(|a| a.title.contains("input connections")),
        "Not has its input"
    );
    let all = actions
        .iter()
        .find(|a| a.title.contains("all missing connections"))
        .expect("an action for the output");
    let fixed = apply(text, &edits_for(all.edit.as_ref().unwrap(), &uri));
    assert!(fixed.contains("    Not(in=s, out=);"), "{fixed}");
}

#[test]
fn hover_on_an_internal_pin_names_its_driver_and_readers() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Blend.hdl");
    let text = "\
CHIP Blend {
    IN a, b;
    OUT out, also;

    PARTS:
    Or(a=a, b=b, out=w);
    Not(in=w, out=out);
    And(a=w, b=a, out=also);
}
";
    h.open(&uri, text);
    let hover = h.hover(&uri, pos_of(text, "w)", 0, 0)).unwrap();
    assert!(hover.contains("internal pin `w` (1 bit)"), "{hover}");
    assert!(hover.contains("driven by `Or` (out)"), "{hover}");
    assert!(hover.contains("read by `Not` (in), `And` (a)"), "{hover}");
}

/// A skeleton as a student has it before the project: no parts.
const SKELETON_XOR: &str = "\
CHIP Xor {
    IN a, b;
    OUT out;

    PARTS:
}
";
const USES_XOR: &str = "\
CHIP Differ {
    IN p, q;
    OUT out;

    PARTS:
    Xor(a=p, b=q, out=out);
}
";

#[test]
fn hint_when_a_local_skeleton_hides_a_built_in_chip() {
    let host = MemoryHost::new()
        .with("/course/02/Xor.hdl", SKELETON_XOR)
        .with(
            "/course/02/Blank.hdl",
            "CHIP Blank {\n    IN a;\n    OUT out;\n    PARTS:\n}\n",
        );
    let mut h = Harness::new(host);
    let uri = url("file:///course/02/Differ.hdl");
    let diagnostics = h.open(&uri, USES_XOR);
    let hint = diagnostics
        .iter()
        .find(|d| {
            d.code
                == Some(lsp::NumberOrString::String(
                    "hdl-skeleton-shadows-builtin".into(),
                ))
        })
        .expect("a hint on the Xor part");
    assert_eq!(hint.severity, Some(lsp::DiagnosticSeverity::HINT));
    assert_eq!(slice(USES_XOR, hint.range), "Xor");
    assert!(
        hint.message.contains("Implement `Xor.hdl`, or move it out"),
        "{}",
        hint.message
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");

    // Not on the skeleton's own file.
    let xor = url("file:///course/02/Xor.hdl");
    let diagnostics = h.open(&xor, SKELETON_XOR);
    assert!(
        !diagnostics.iter().any(|d| d.message.contains("hides the")),
        "{diagnostics:?}"
    );
    // Not for a skeleton that hides nothing.
    let other = url("file:///course/02/Wrap.hdl");
    let diagnostics = h.open(
        &other,
        "CHIP Wrap {\n    IN a;\n    OUT out;\n    PARTS:\n    Blank(a=a, out=out);\n}\n",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // Once the file is no longer an empty skeleton (here it delegates to
    // the built-in), the hint goes away.
    let published = h.change_all(
        &xor,
        "CHIP Xor {\n    IN a, b;\n    OUT out;\n\n    BUILTIN Xor;\n}\n",
    );
    let differ = published
        .iter()
        .rev()
        .find(|(u, _)| *u == uri)
        .expect("Differ is refreshed when Xor changes");
    assert!(differ.1.is_empty(), "{:?}", differ.1);
}

#[test]
fn half_typed_chip_reports_the_syntax_error_only() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Foo.hdl");
    let text = "CHIP Foo { IN a; PARTS: And(a=";
    let diagnostics = h.open(&uri, text);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].range.start, lsp::Position::new(0, 30));
    let end = lsp::Position::new(0, 30);
    assert!(!h.labels(&uri, end).is_empty(), "completion still works");
    assert!(
        h.hover(&uri, lsp::Position::new(0, 25)).is_some(),
        "hover on And"
    );
    assert!(h.signature_help(&uri, end).is_some());
}
