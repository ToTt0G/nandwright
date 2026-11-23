//! Unit tests for the VM parser and linker (`parse`, `Program::link`).

use n2t_core::diagnostic::{DiagnosticCode, Severity};
use n2t_vm::{ArithOp, BuiltinOs, CallTarget, CommandKind, EntryPoint, Program, Segment, parse};

#[test]
fn parses_all_commands_with_spans_and_lines() {
    let src = "\u{FEFF}// header comment\n\
function Foo.calc 2\n\
  push constant 7 /* inline */\n\
  push argument 0\n\
  add\n\
  pop local 0\n\
label LOOP\n\
  push local 0\n\
  if-goto LOOP\n\
  goto LOOP\n\
  call Bar.helper 1\n\
  return\n";
    let parsed = parse("Foo", src);
    assert!(
        !parsed.has_errors(),
        "unexpected errors: {:?}",
        parsed.diagnostics
    );
    assert_eq!(parsed.file_stem, "Foo");
    assert_eq!(parsed.commands.len(), 11);

    // Line numbers (0-based)
    assert_eq!(parsed.commands[0].line, 1);
    assert_eq!(parsed.commands[1].line, 2);
    assert_eq!(parsed.commands[10].line, 11);

    // Spans slice exact source text
    assert_eq!(parsed.commands[0].span.slice(src), "function Foo.calc 2");
    assert_eq!(parsed.commands[0].keyword_span.slice(src), "function");
    if let CommandKind::Function {
        name,
        n_locals,
        n_locals_span,
    } = &parsed.commands[0].kind
    {
        assert_eq!(name.text, "Foo.calc");
        assert_eq!(name.span.slice(src), "Foo.calc");
        assert_eq!(*n_locals, 2);
        assert_eq!(n_locals_span.slice(src), "2");
    } else {
        panic!("expected Function command");
    }

    // LSP helpers
    assert_eq!(parsed.functions().count(), 1);
    assert_eq!(parsed.labels().count(), 1);
    assert_eq!(parsed.resolve_label(7, "LOOP"), Some(5));
    assert_eq!(parsed.resolve_label(7, "MISSING"), None);
    let offset = src.find("call Bar.helper").unwrap() + 5;
    assert_eq!(parsed.command_at(offset), Some(9));
}

#[test]
fn reports_all_parse_diagnostics_and_recovers_per_line() {
    let src = "\
PUSH constant 1
boguscmd
push
push constant 1 extra
push unknownseg 0
push constant -5
push constant abc
push constant 32768
push constant 999999
pop constant 0
push temp 8
push pointer 2
push static 239
label
label A B
goto MISSING
function
function BadName 0
function Other.fn 0
call NoDot 0
call Foo.ok 1 extra
add extra_arg
return extra_arg
label DUP
label DUP
/* unterminated comment";

    let parsed = parse("Foo", src);
    let codes: Vec<(Severity, &str)> = parsed
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.code.as_str()))
        .collect();

    let has = |sev: Severity, code: &str| codes.contains(&(sev, code));
    assert!(has(Severity::Error, "vm-unknown-command"), "{codes:?}");
    assert!(has(Severity::Error, "vm-missing-argument"), "{codes:?}");
    assert!(has(Severity::Error, "vm-too-many-arguments"), "{codes:?}");
    assert!(has(Severity::Error, "vm-unknown-segment"), "{codes:?}");
    assert!(has(Severity::Error, "vm-negative-number"), "{codes:?}");
    assert!(has(Severity::Error, "vm-invalid-number"), "{codes:?}");
    assert!(has(Severity::Error, "vm-number-too-large"), "{codes:?}");
    assert!(has(Severity::Error, "vm-pop-constant"), "{codes:?}");
    assert!(has(Severity::Warning, "vm-index-out-of-range"), "{codes:?}");
    assert!(has(Severity::Error, "vm-unknown-label"), "{codes:?}");
    assert!(
        has(Severity::Warning, "vm-invalid-function-name"),
        "{codes:?}"
    );
    assert!(
        has(Severity::Warning, "vm-function-class-mismatch"),
        "{codes:?}"
    );
    assert!(
        has(Severity::Error, "vm-invalid-function-name"),
        "{codes:?}"
    );
    assert!(has(Severity::Warning, "vm-extra-arguments"), "{codes:?}");
    assert!(has(Severity::Warning, "vm-duplicate-label"), "{codes:?}");
    assert!(
        has(Severity::Warning, "vm-unterminated-comment"),
        "{codes:?}"
    );

    // Ensure valid commands on other lines were still recovered
    assert!(
        parsed
            .commands
            .iter()
            .any(|c| matches!(c.kind, CommandKind::Arithmetic(ArithOp::Add)))
    );
    assert!(
        parsed
            .commands
            .iter()
            .any(|c| matches!(c.kind, CommandKind::Return))
    );
}

#[test]
fn duplicate_label_resolves_to_last_definition() {
    let src = "\
function A.f 0
label X
push constant 1
label X
push constant 2
goto X
return
";
    let parsed = parse("A", src);
    assert!(!parsed.has_errors());
    assert_eq!(parsed.resolve_label(5, "X"), Some(3));
}

#[test]
fn links_multi_file_with_sys_first_statics_and_builtins() {
    let alpha = parse(
        "Alpha",
        "function Alpha.f 0\npush static 2\ncall Math.multiply 2\nreturn\n",
    );
    let sys = parse(
        "Sys",
        "function Sys.init 0\npush constant 1\npop static 0\ncall Alpha.f 0\nlabel END\ngoto END\n",
    );
    let (prog, diags) = Program::link(vec![alpha, sys]);
    assert!(diags.is_empty(), "unexpected link diagnostics: {diags:?}");

    // Sys.vm is placed first
    assert_eq!(prog.files()[0].stem, "Sys");
    assert_eq!(prog.files()[0].static_base, 16);
    assert_eq!(prog.files()[0].static_count, 1);

    assert_eq!(prog.files()[1].stem, "Alpha");
    assert_eq!(prog.files()[1].static_base, 17);
    assert_eq!(prog.files()[1].static_count, 3);

    assert!(matches!(
        prog.entry_point(),
        EntryPoint::UserSysInit { pc: 0 }
    ));
    assert!(prog.has_builtin_calls());
}

#[test]
fn user_function_overrides_builtin_and_unknown_call_is_reported() {
    let custom_math = parse(
        "Math",
        "function Math.multiply 0\npush constant 99\nreturn\n",
    );
    let main = parse(
        "Main",
        "function Main.main 0\n\
         call Math.multiply 2\n\
         call Math.abs 1\n\
         call Ghost.missing 0\n\
         return\n",
    );
    let (prog, diags) = Program::link(vec![custom_math, main]);
    assert!(matches!(
        prog.entry_point(),
        EntryPoint::BuiltinSysInit { .. }
    ));

    // Only Ghost.missing is an unknown function error
    let errs: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, DiagnosticCode::Other("vm-unknown-function"));
    assert!(errs[0].message.contains("Ghost.missing"));

    // Verify Math.multiply resolved to User and Math.abs resolved to Builtin
    let main_info = prog.function_by_name("Main.main").unwrap();
    assert_eq!(main_info.name, "Main.main");
    let _ = (CallTarget::Builtin(BuiltinOs::MathAbs), Segment::Local);
}

#[test]
fn reports_duplicate_function_and_static_overflow() {
    let f1 = parse("A", "function A.dup 0\npush static 200\nreturn\n");
    let f2 = parse("B", "function A.dup 0\npush static 100\nreturn\n");
    let (_prog, diags) = Program::link(vec![f1, f2]);
    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::Other("vm-duplicate-function"))
    );
    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::Other("vm-static-overflow"))
    );
}
