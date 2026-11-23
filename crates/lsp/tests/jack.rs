//! Jack (`.jack`) documents: per-file and folder-level features.

mod common;

use common::{Harness, apply, edits_for, pos_of, slice, url};
use lsp_types as lsp;
use n2t_core::host::MemoryHost;

const DIR: &str = "c:/Users/me/My Chips";
const MAIN_URI: &str = "file:///c%3A/Users/me/My%20Chips/Main.jack";
const TALLY_URL: &str = "file:///c:/Users/me/My%20Chips/Tally.jack";

/// An original class: counts events in steps.
const TALLY: &str = "\
/** Counts events in steps. */
class Tally {
    field int count, step;
    static int instances;

    /** Makes a tally that adds `by` per event. */
    constructor Tally new(int by) {
        let count = 0;
        let step = by;
        let instances = instances + 1;
        return this;
    }

    /** Records one event. */
    method void bump() {
        let count = count + step;
        return;
    }

    method int total() {
        return count;
    }

    function int made() {
        return instances;
    }

    method void dispose() {
        do Memory.deAlloc(this);
        return;
    }
}
";

const MAIN: &str = "\
class Main {
    function void main() {
        var Tally t;
        var int i;
        let t = Tally.new(3);
        let i = 0;
        while (i < 4) {
            do t.bump();
            let i = i + 1;
        }
        do Output.printInt(t.total());
        do Output.printInt(Math.max(t.total(), Tally.made()));
        do t.dispose();
        return;
    }
}
";

fn folder() -> (Harness, lsp::Url) {
    let host = MemoryHost::new().with(&format!("{DIR}/Tally.jack"), TALLY);
    let mut h = Harness::new(host);
    let uri = url(MAIN_URI);
    let diagnostics = h.open(&uri, MAIN);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (h, uri)
}

#[test]
fn folder_diagnostics_check_calls_between_classes() {
    let (mut h, uri) = folder();
    // A call to a subroutine the other class does not have.
    let text = MAIN.replace("do t.bump();", "do t.bumpp();");
    let diagnostics = h.change(&uri, &text);
    let error = diagnostics
        .iter()
        .find(|d| d.severity == Some(lsp::DiagnosticSeverity::ERROR))
        .expect("an error for the unknown method");
    assert_eq!(slice(&text, error.range), "bumpp");
    // The wrong number of arguments.
    let text = MAIN.replace("Tally.new(3)", "Tally.new(3, 4)");
    let diagnostics = h.change(&uri, &text);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.severity == Some(lsp::DiagnosticSeverity::ERROR) && d.range.start.line == 4),
        "{diagnostics:?}"
    );
}

#[test]
fn open_buffers_win_over_files_on_disk() {
    // On disk, Tally has no `total`; the open buffer does.
    let old = TALLY.replace("method int total()", "method int sum()");
    let host = MemoryHost::new().with(&format!("{DIR}/Tally.jack"), &old);
    let mut h = Harness::new(host);
    let main = url(MAIN_URI);
    let diagnostics = h.open(&main, MAIN);
    assert!(
        diagnostics.iter().any(|d| d.message.contains("total")),
        "{diagnostics:?}"
    );
    let tally = url(TALLY_URL);
    h.open(&tally, TALLY);
    let published = h.published();
    let _ = published;
    let diagnostics = h.change(&main, MAIN);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn syntax_errors_and_unreachable_code_are_placed_precisely() {
    let mut h = Harness::empty();
    let uri = url("file:///work/Gauge.jack");
    let text = "class Gauge {\n    function int level() {\n        return 1;\n        let x = ;\n    }\n}\n";
    let diagnostics = h.open(&uri, text);
    assert!(!diagnostics.is_empty());
    for d in &diagnostics {
        assert_ne!(d.range.start, lsp::Position::new(0, 0), "{d:?}");
    }
    let text = "class Gauge {\n    function int level() {\n        return 1;\n        do Output.println();\n    }\n}\n";
    let diagnostics = h.change(&uri, text);
    let unreachable = diagnostics
        .iter()
        .find(|d| d.code == Some(lsp::NumberOrString::String("jack-unreachable-code".into())))
        .expect("unreachable code");
    assert_eq!(unreachable.range.start.line, 3);
    assert_eq!(
        unreachable.tags.as_deref(),
        Some(&[lsp::DiagnosticTag::UNNECESSARY][..])
    );
}

#[test]
fn hover_shows_kind_type_and_vm_location() {
    let (mut h, uri) = folder();
    let tally = url(TALLY_URL);
    h.open(&tally, TALLY);
    let hover = h
        .hover(&tally, pos_of(TALLY, "count + step", 0, 0))
        .unwrap();
    assert!(hover.contains("field int count"), "{hover}");
    assert!(hover.contains("`this 0`"), "{hover}");
    let hover = h.hover(&tally, pos_of(TALLY, "step;\n", 1, 0)).unwrap();
    assert!(hover.contains("`this 1`"), "{hover}");
    let hover = h
        .hover(&tally, pos_of(TALLY, "instances + 1", 0, 0))
        .unwrap();
    assert!(
        hover.contains("static int instances") && hover.contains("`static 0`"),
        "{hover}"
    );
    let hover = h.hover(&tally, pos_of(TALLY, "= by", 0, 2)).unwrap();
    assert!(
        hover.contains("argument int by") && hover.contains("`argument 0`"),
        "{hover}"
    );

    let hover = h.hover(&uri, pos_of(MAIN, "i = 0", 0, 0)).unwrap();
    assert!(
        hover.contains("var int i") && hover.contains("`local 1`"),
        "{hover}"
    );
    let hover = h.hover(&uri, pos_of(MAIN, "new(3)", 0, 0)).unwrap();
    assert!(
        hover.contains("constructor Tally Tally.new(int by)"),
        "{hover}"
    );
    assert!(hover.contains("Makes a tally"), "{hover}");
    let hover = h.hover(&uri, pos_of(MAIN, "max(", 0, 0)).unwrap();
    assert!(
        hover.contains("function int Math.max(int x, int y)"),
        "{hover}"
    );
    assert!(hover.contains("The larger of x and y."), "{hover}");
    let hover = h.hover(&uri, pos_of(MAIN, "Output", 0, 1)).unwrap();
    assert!(hover.contains("Jack OS class"), "{hover}");
    let hover = h.hover(&uri, pos_of(MAIN, "bump", 0, 1)).unwrap();
    assert!(hover.contains("method void Tally.bump()"), "{hover}");
    assert!(hover.contains("Records one event."), "{hover}");
}

#[test]
fn definition_and_references_cross_files() {
    let (mut h, uri) = folder();
    let def = h.definition(&uri, pos_of(MAIN, "bump", 0, 1)).unwrap();
    assert_eq!(def.uri.as_str(), TALLY_URL);
    assert_eq!(def.range.start, pos_of(TALLY, "bump()", 0, 0));
    let def = h.definition(&uri, pos_of(MAIN, "Tally t", 0, 1)).unwrap();
    assert_eq!(def.range.start, pos_of(TALLY, "Tally {", 0, 0));
    let def = h.definition(&uri, pos_of(MAIN, "i < 4", 0, 0)).unwrap();
    assert_eq!(def.uri, uri);
    assert_eq!(def.range.start, pos_of(MAIN, "i;", 0, 0));
    // OS subroutines have no source.
    assert_eq!(h.definition(&uri, pos_of(MAIN, "max", 0, 0)), None);

    let refs = h.references(&uri, pos_of(MAIN, "total", 0, 0));
    assert_eq!(refs.len(), 3, "a declaration and two calls: {refs:?}");
    assert_eq!(
        refs.iter().filter(|l| l.uri.as_str() == TALLY_URL).count(),
        1
    );
    let refs = h.references(&uri, pos_of(MAIN, "Tally", 0, 0));
    assert_eq!(refs.len(), 5, "{refs:?}");
    let highlights = h.highlights(&uri, pos_of(MAIN, "t = ", 0, 0));
    assert_eq!(highlights.len(), 6, "{highlights:?}");
}

#[test]
fn rename_variables_subroutines_and_classes() {
    let (mut h, uri) = folder();
    // A local, within the file.
    let edit = h
        .rename(&uri, pos_of(MAIN, "i < 4", 0, 0), "k")
        .unwrap()
        .unwrap();
    let renamed = apply(MAIN, &edits_for(&edit, &uri));
    assert!(renamed.contains("var int k;") && renamed.contains("let k = k + 1;"));
    assert!(renamed.contains("while (k < 4)"));
    // Refused: collisions, keywords, class names, the OS.
    let at_i = pos_of(MAIN, "i < 4", 0, 0);
    assert!(
        h.rename(&uri, at_i, "t")
            .unwrap_err()
            .contains("already visible")
    );
    assert!(
        h.rename(&uri, at_i, "while")
            .unwrap_err()
            .contains("not a valid")
    );
    assert!(
        h.rename(&uri, at_i, "Math")
            .unwrap_err()
            .contains("class name")
    );
    assert!(
        h.rename(&uri, pos_of(MAIN, "max", 0, 0), "biggest")
            .unwrap_err()
            .contains("Jack OS")
    );
    assert!(h.rename(&uri, pos_of(MAIN, "Output", 0, 0), "Out").is_err());

    // A method, across files.
    let edit = h
        .rename(&uri, pos_of(MAIN, "bump", 0, 0), "tick")
        .unwrap()
        .unwrap();
    let tally = url(TALLY_URL);
    assert!(apply(MAIN, &edits_for(&edit, &uri)).contains("do t.tick();"));
    assert!(apply(TALLY, &edits_for(&edit, &tally)).contains("method void tick()"));
    assert!(
        h.rename(&uri, pos_of(MAIN, "bump", 0, 0), "total")
            .unwrap_err()
            .contains("already has")
    );

    // A class: every use, and the file is renamed to match.
    let edit = h
        .rename(&uri, pos_of(MAIN, "Tally.new", 0, 0), "Counter")
        .unwrap()
        .unwrap();
    let main = apply(MAIN, &edits_for(&edit, &uri));
    assert!(main.contains("var Counter t;") && main.contains("Counter.new(3)"));
    assert!(apply(TALLY, &edits_for(&edit, &tally)).contains("class Counter {"));
    let Some(lsp::DocumentChanges::Operations(ops)) = &edit.document_changes else {
        panic!("expected document changes");
    };
    let rename = ops.iter().find_map(|op| match op {
        lsp::DocumentChangeOperation::Op(lsp::ResourceOp::Rename(r)) => Some(r),
        _ => None,
    });
    let rename = rename.expect("a file rename");
    assert_eq!(rename.old_uri.as_str(), TALLY_URL);
    assert_eq!(
        rename.new_uri.as_str(),
        "file:///c:/Users/me/My%20Chips/Counter.jack"
    );
    assert!(
        h.rename(&uri, pos_of(MAIN, "Tally.new", 0, 0), "Main")
            .unwrap_err()
            .contains("already exists")
    );
}

#[test]
fn completion_in_a_half_typed_file() {
    let (mut h, uri) = folder();
    // Everything after the cursor is missing: the file ends mid-statement.
    let cut = |marker: &str, extra: &str| {
        let at = MAIN.find(marker).unwrap();
        format!("{}{extra}", &MAIN[..at])
    };
    let mut ask = |text: String| {
        h.change(&uri, &text);
        let line = text.matches('\n').count() as u32;
        let column = text.rsplit('\n').next().unwrap().len() as u32;
        h.labels(&uri, lsp::Position::new(line, column))
    };

    let methods = ask(cut("do t.bump", "do t."));
    assert!(
        methods.contains(&"bump".into()) && methods.contains(&"total".into()),
        "{methods:?}"
    );
    assert!(
        !methods.contains(&"made".into()) && !methods.contains(&"new".into()),
        "{methods:?}"
    );

    let functions = ask(cut("do t.bump", "do Tally."));
    assert_eq!(functions, ["new", "made"]);

    let os = ask(cut("do t.bump", "do Math."));
    assert!(
        os.contains(&"max".into()) && os.contains(&"sqrt".into()),
        "{os:?}"
    );
    assert!(
        !os.contains(&"init".into()),
        "internal routines are hidden: {os:?}"
    );

    let statements = ask(cut("do t.bump", ""));
    for k in ["let", "do", "if", "while", "return"] {
        assert!(statements.contains(&k.into()), "{statements:?}");
    }
    assert!(
        !statements.contains(&"var".into()),
        "var only before statements"
    );
    let first = ask(cut("var Tally t", ""));
    assert!(first.contains(&"var".into()), "{first:?}");

    let types = ask(cut("var Tally t", "var "));
    for t in ["int", "char", "boolean", "Tally", "Array", "String"] {
        assert!(types.contains(&t.into()), "{t}: {types:?}");
    }

    let targets = ask(cut("let i = 0", "let "));
    assert!(
        targets.contains(&"t".into()) && targets.contains(&"i".into()),
        "{targets:?}"
    );

    let values = ask(cut("let i = 0", "let i = "));
    for v in ["t", "i", "true", "false", "null", "Tally", "Math"] {
        assert!(values.contains(&v.into()), "{v}: {values:?}");
    }
    assert!(!values.contains(&"this".into()), "no `this` in a function");

    let prefix = ask(cut("let i = 0", "let i = Ma"));
    assert!(prefix.contains(&"Math".into()), "{prefix:?}");

    let class_level = ask("class Main {\n    ".to_string());
    for k in ["field", "static", "function", "method", "constructor"] {
        assert!(class_level.contains(&k.into()), "{class_level:?}");
    }
    assert_eq!(ask(String::new()), ["class"]);
    assert!(
        ask(cut("let i = 0", "// let i = ")).is_empty(),
        "in a comment"
    );
    assert!(
        ask(cut("let i = 0", "do Output.printString(\"ab")).is_empty(),
        "in a string"
    );
}

#[test]
fn signature_help_for_user_and_os_calls() {
    let (mut h, uri) = folder();
    let help = h
        .signature_help(&uri, pos_of(MAIN, "Math.max(", 0, 9))
        .unwrap();
    assert_eq!(
        help.signatures[0].label,
        "function int Math.max(int x, int y)"
    );
    assert_eq!(help.active_parameter, Some(0));
    let help = h
        .signature_help(&uri, pos_of(MAIN, "Tally.made()", 0, 0))
        .unwrap();
    assert_eq!(
        help.signatures[0].label,
        "function int Math.max(int x, int y)"
    );
    assert_eq!(help.active_parameter, Some(1));
    let help = h
        .signature_help(&uri, pos_of(MAIN, "new(3)", 0, 4))
        .unwrap();
    assert_eq!(
        help.signatures[0].label,
        "constructor Tally Tally.new(int by)"
    );
    let help = h
        .signature_help(&uri, pos_of(MAIN, "bump()", 0, 5))
        .unwrap();
    assert_eq!(help.signatures[0].label, "method void Tally.bump()");
    assert_eq!(help.active_parameter, None);
    assert!(
        h.signature_help(&uri, pos_of(MAIN, "i < 4", 0, 0))
            .is_none()
    );
    assert!(
        h.signature_help(&uri, pos_of(MAIN, "main()", 0, 5))
            .is_none()
    );
}

#[test]
fn document_symbols_and_semantic_tokens() {
    let (mut h, _) = folder();
    let tally = url(TALLY_URL);
    h.open(&tally, TALLY);
    let symbols = h.symbols(&tally);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "Tally");
    assert_eq!(symbols[0].kind, lsp::SymbolKind::CLASS);
    let children: Vec<(String, lsp::SymbolKind)> = symbols[0]
        .children
        .iter()
        .flatten()
        .map(|c| (c.name.clone(), c.kind))
        .collect();
    assert_eq!(
        children,
        [
            ("count".to_string(), lsp::SymbolKind::FIELD),
            ("step".to_string(), lsp::SymbolKind::FIELD),
            ("instances".to_string(), lsp::SymbolKind::VARIABLE),
            ("new".to_string(), lsp::SymbolKind::CONSTRUCTOR),
            ("bump".to_string(), lsp::SymbolKind::METHOD),
            ("total".to_string(), lsp::SymbolKind::METHOD),
            ("made".to_string(), lsp::SymbolKind::FUNCTION),
            ("dispose".to_string(), lsp::SymbolKind::METHOD),
        ]
    );

    // Decode the tokens back to (line, column, length, type).
    let legend = n2t_lsp::capabilities();
    let Some(lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(options)) =
        legend.semantic_tokens_provider
    else {
        panic!("no legend");
    };
    let types = options.legend.token_types;
    let mut decoded = Vec::new();
    let (mut line, mut col) = (0, 0);
    for t in h.semantic_tokens(&tally) {
        if t.delta_line > 0 {
            line += t.delta_line;
            col = t.delta_start;
        } else {
            col += t.delta_start;
        }
        let pos = lsp::Position::new(line, col);
        let range = lsp::Range::new(pos, lsp::Position::new(line, col + t.length));
        decoded.push((
            slice(TALLY, range).to_string(),
            types[t.token_type as usize].as_str().to_string(),
        ));
    }
    let has = |name: &str, ty: &str| decoded.iter().any(|(n, t)| n == name && t == ty);
    assert!(has("Tally", "class"), "{decoded:?}");
    assert!(has("count", "property"), "{decoded:?}");
    assert!(has("instances", "variable"), "{decoded:?}");
    assert!(has("by", "parameter"), "{decoded:?}");
    assert!(has("bump", "method"), "{decoded:?}");
    assert!(has("made", "function"), "{decoded:?}");
    assert!(has("Memory", "class"), "{decoded:?}");
}
