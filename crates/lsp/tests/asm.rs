//! Hack assembly (`.asm`) and machine code (`.hack`) documents.

mod common;

use common::{Harness, apply, edits_for, pos_of, slice, url};
use lsp_types as lsp;

/// An original program: writes n, n-1, …, 1 into RAM[100] onward.
const COUNTDOWN: &str = "\
// Writes 10, 9, ..., 1 into RAM[100] onward.
    @10
    D=A
    @n
    M=D
    @100
    D=A
    @ptr
    M=D
(LOOP)
    @n
    D=M
    @END
    D;JEQ
    @ptr
    A=M
    M=D
    @ptr
    M=M+1
    @n
    M=M-1
    @LOOP
    0;JMP
(END)
    @END
    0;JMP
";

const WINDOWS_URI: &str = "file:///c%3A/Users/me/My%20Chips/Countdown.asm";

fn open_countdown() -> (Harness, lsp::Url) {
    let mut h = Harness::empty();
    let uri = url(WINDOWS_URI);
    let diagnostics = h.open(&uri, COUNTDOWN);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (h, uri)
}

#[test]
fn asm_diagnostics_point_at_the_problem() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Bad.asm");
    let text = "@1\nD=Q\n@-1\n";
    let diagnostics = h.open(&uri, text);
    let error = diagnostics
        .iter()
        .find(|d| d.severity == Some(lsp::DiagnosticSeverity::ERROR))
        .expect("an error for `D=Q`");
    assert_eq!(error.range.start.line, 1);
    assert_eq!(slice(text, error.range), "Q");
    // `@-1` is accepted by the official assembler but surprising: a warning.
    assert!(
        diagnostics.iter().any(
            |d| d.range.start.line == 2 && d.severity == Some(lsp::DiagnosticSeverity::WARNING)
        ),
        "{diagnostics:?}"
    );
}

#[test]
fn hack_diagnostics_name_the_bad_line() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Prog.hack");
    let diagnostics = h.open(&uri, "0000000000000010\n111011000001000\n");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].range.start.line, 1);
}

#[test]
fn hover_shows_machine_code_address_and_meaning() {
    let (mut h, uri) = open_countdown();
    // `@n`: the variable and the instruction.
    let hover = h.hover(&uri, pos_of(COUNTDOWN, "@n", 0, 1)).unwrap();
    assert!(hover.contains("**variable** `n` at RAM[16]"), "{hover}");
    assert!(hover.contains("`0000000000010000` at ROM[2]"), "{hover}");
    // `@ptr` is the second variable.
    let hover = h.hover(&uri, pos_of(COUNTDOWN, "@ptr", 0, 2)).unwrap();
    assert!(hover.contains("RAM[17]"), "{hover}");
    // A label definition: its ROM address.
    let hover = h.hover(&uri, pos_of(COUNTDOWN, "(LOOP)", 0, 2)).unwrap();
    assert!(hover.contains("**label** `LOOP` = ROM[8]"), "{hover}");
    // A C-instruction in words.
    let hover = h.hover(&uri, pos_of(COUNTDOWN, "D;JEQ", 0, 0)).unwrap();
    assert!(hover.contains("jump to A if D = 0"), "{hover}");
    assert!(hover.contains("`1110001100000010` at ROM[11]"), "{hover}");
}

#[test]
fn hover_describes_dest_comp_jump_in_words() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Words.asm");
    let text = "@SCREEN\nD=M+1;JGT\nAM=!M\nD|A\n";
    h.open(&uri, text);
    let hover = h.hover(&uri, lsp::Position::new(1, 3)).unwrap();
    assert!(
        hover.contains("D = RAM[A] + 1, then jump to A if the result > 0"),
        "{hover}"
    );
    let hover = h.hover(&uri, lsp::Position::new(2, 1)).unwrap();
    assert!(hover.contains("A, RAM[A] = NOT RAM[A]"), "{hover}");
    let hover = h.hover(&uri, lsp::Position::new(3, 1)).unwrap();
    assert!(hover.contains("compute D OR A"), "{hover}");
    let hover = h.hover(&uri, lsp::Position::new(0, 3)).unwrap();
    assert!(
        hover.contains("**predefined symbol** `SCREEN` = 16384"),
        "{hover}"
    );
}

#[test]
fn hack_hover_disassembles_the_line() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Prog.hack");
    h.open(&uri, "0000000000000111\n1110011111001001\n");
    let hover = h.hover(&uri, lsp::Position::new(0, 4)).unwrap();
    assert!(hover.contains("ROM[0]: `@7`"), "{hover}");
    let hover = h.hover(&uri, lsp::Position::new(1, 4)).unwrap();
    assert!(hover.contains("ROM[1]: `M=D+1;JGT`"), "{hover}");
    assert!(
        hover.contains("RAM[A] = D + 1, then jump to A if the result > 0"),
        "{hover}"
    );
    assert_eq!(h.hover(&uri, lsp::Position::new(5, 0)), None);
}

#[test]
fn definition_references_and_highlights_for_labels_and_variables() {
    let (mut h, uri) = open_countdown();
    let def = h
        .definition(&uri, pos_of(COUNTDOWN, "@LOOP", 0, 2))
        .unwrap();
    assert_eq!(def.uri, uri);
    assert_eq!(def.range.start, pos_of(COUNTDOWN, "LOOP)", 0, 0));

    let refs = h.references(&uri, pos_of(COUNTDOWN, "(END)", 0, 1));
    assert_eq!(refs.len(), 3, "definition and two uses: {refs:?}");

    let highlights = h.highlights(&uri, pos_of(COUNTDOWN, "@n", 1, 1));
    assert_eq!(highlights.len(), 3, "{highlights:?}");
    // A variable's definition is its first use, which is a read of the name.
    let highlights = h.highlights(&uri, pos_of(COUNTDOWN, "LOOP)", 0, 0));
    let writes = highlights
        .iter()
        .filter(|h| h.kind == Some(lsp::DocumentHighlightKind::WRITE))
        .count();
    assert_eq!(writes, 1);
    // Predefined symbols have no definition in the file.
    let mut h2 = Harness::empty();
    let uri2 = url("file:///work/P.asm");
    h2.open(&uri2, "@KBD\nD=M\n");
    assert_eq!(h2.definition(&uri2, lsp::Position::new(0, 2)), None);
}

#[test]
fn rename_keeps_the_program_meaning_and_refuses_collisions() {
    let (mut h, uri) = open_countdown();
    let before = n2t_hack::assemble(COUNTDOWN).words;

    // `n` (RAM[16]) renamed to `zcount`, which sorts after `ptr` (RAM[17]):
    // allocation is by first use, not by name, so the machine code is the same.
    let edit = h
        .rename(&uri, pos_of(COUNTDOWN, "@n", 0, 1), "zcount")
        .unwrap()
        .unwrap();
    let renamed = apply(COUNTDOWN, &edits_for(&edit, &uri));
    assert_eq!(renamed.matches("@zcount").count(), 3);
    assert!(!renamed.contains("@n\n"));
    assert_eq!(n2t_hack::assemble(&renamed).words, before);

    // A label, with its definition.
    let edit = h
        .rename(&uri, pos_of(COUNTDOWN, "@END", 0, 2), "DONE")
        .unwrap()
        .unwrap();
    let renamed = apply(COUNTDOWN, &edits_for(&edit, &uri));
    assert!(renamed.contains("(DONE)") && renamed.contains("@DONE"));
    assert_eq!(n2t_hack::assemble(&renamed).words, before);

    // Refused: collisions, predefined symbols, invalid names.
    let at_n = pos_of(COUNTDOWN, "@n", 0, 1);
    assert!(
        h.rename(&uri, at_n, "ptr")
            .unwrap_err()
            .contains("already exists")
    );
    assert!(
        h.rename(&uri, at_n, "LOOP")
            .unwrap_err()
            .contains("already exists")
    );
    assert!(
        h.rename(&uri, at_n, "R5")
            .unwrap_err()
            .contains("predefined")
    );
    assert!(
        h.rename(&uri, at_n, "1x")
            .unwrap_err()
            .contains("not a valid")
    );
    assert!(h.rename(&uri, at_n, "a b").is_err());
    let mut h2 = Harness::empty();
    let uri2 = url("file:///work/P.asm");
    h2.open(&uri2, "@SCREEN\nM=-1\n");
    assert!(
        h2.rename(&uri2, lsp::Position::new(0, 2), "PIXELS")
            .unwrap_err()
            .contains("predefined")
    );
    let prepared = h2.request::<lsp_types::request::PrepareRenameRequest>(common::position(
        &uri2,
        lsp::Position::new(0, 2),
    ));
    assert_eq!(prepared, None);
}

#[test]
fn completion_offers_symbols_after_at_and_mnemonics_in_place() {
    let mut h = Harness::empty();
    let uri = url(WINDOWS_URI);
    let text = format!("{COUNTDOWN}    @\n    D=\n    D;J\n    \n");
    h.open(&uri, &text);
    let last = COUNTDOWN.lines().count() as u32;

    let labels = h.labels(&uri, lsp::Position::new(last, 5));
    for expected in ["LOOP", "END", "n", "ptr", "SCREEN", "KBD", "R13", "SP"] {
        assert!(
            labels.contains(&expected.to_string()),
            "{expected}: {labels:?}"
        );
    }
    let comps = h.labels(&uri, lsp::Position::new(last + 1, 6));
    assert!(comps.contains(&"M+1".to_string()), "{comps:?}");
    assert!(comps.contains(&"D|M".to_string()), "{comps:?}");
    assert!(!comps.contains(&"JMP".to_string()));
    let jumps = h.labels(&uri, lsp::Position::new(last + 2, 7));
    assert_eq!(jumps.len(), 7, "{jumps:?}");
    assert!(jumps.contains(&"JGT".to_string()));
    let start = h.labels(&uri, lsp::Position::new(last + 3, 4));
    assert!(start.contains(&"AMD=".to_string()), "{start:?}");
    assert!(start.contains(&"0".to_string()), "{start:?}");
    // In a comment: nothing.
    assert!(h.labels(&uri, lsp::Position::new(0, 5)).is_empty());

    // The replaced range covers the half-typed symbol after `@`.
    let mut h = Harness::empty();
    let uri = url("file:///work/Half.asm");
    h.open(&uri, "(LOOP)\n@LO\n");
    let items = h.completion(&uri, lsp::Position::new(1, 3));
    let item = items.iter().find(|i| i.label == "LOOP").unwrap();
    let Some(lsp::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("no edit");
    };
    assert_eq!(edit.range.start, lsp::Position::new(1, 1));
    assert!(
        !items.iter().any(|i| i.label == "LO"),
        "the word being typed"
    );
}

#[test]
fn document_symbols_and_folding_by_label() {
    let (mut h, uri) = open_countdown();
    let symbols = h.symbols(&uri);
    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["LOOP", "END"]);
    assert_eq!(symbols[0].detail.as_deref(), Some("ROM[8]"));
    assert_eq!(symbols[0].range.start.line, 9);
    assert_eq!(symbols[0].range.end.line, 22);
    let folds = h.request::<lsp_types::request::FoldingRangeRequest>(lsp::FoldingRangeParams {
        text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    });
    let folds = folds.unwrap();
    assert_eq!(folds.len(), 2);
    assert_eq!((folds[0].start_line, folds[0].end_line), (9, 22));
}
