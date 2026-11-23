//! VM files (`.vm`): diagnostics while typing.

mod common;

use common::Harness;
use lsp_types as lsp;

#[test]
fn vm_diagnostics_point_at_the_problem() {
    let mut h = Harness::empty();
    let uri = lsp::Url::parse("file:///c%3A/Users/me/My%20Programs/Adder.vm").unwrap();
    let text =
        "function Adder.sum 1\npush constant 7\npush locall 0\nadd\npop constant 3\nreturn\n";
    let diagnostics = h.open(&uri, text);
    let lines: Vec<u32> = diagnostics
        .iter()
        .filter(|d| d.severity == Some(lsp::DiagnosticSeverity::ERROR))
        .map(|d| d.range.start.line)
        .collect();
    // An unknown segment and a pop into the constant segment.
    assert_eq!(lines, [2, 4], "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.range.start != lsp::Position::new(0, 0))
    );

    // A correct file is clean, and so is a half-typed one's valid part.
    let clean = h.open(
        &lsp::Url::parse("file:///work/Ok.vm").unwrap(),
        "function Ok.run 0\npush constant 1\nreturn\n",
    );
    assert!(clean.is_empty(), "{clean:?}");
}
