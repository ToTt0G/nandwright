//! Behaviour of the public API. Expected VM code and line numbers below were
//! observed from the official JackCompiler on these (original) snippets.

use n2t_jack::ast::{Statement, Term};
use n2t_jack::{Role, TokenKind, VarKind, compile, compile_program, os, parse, resolve};

fn vm(src: &str) -> String {
    let c = compile("Main", src);
    assert!(!c.has_errors(), "{:#?}", c.diagnostics);
    c.vm.unwrap()
}

/// `(official line, code)` of every error.
fn errors(src: &str) -> Vec<(u32, &'static str)> {
    let c = compile("Main", src);
    c.diagnostics
        .iter()
        .zip(&c.official_lines)
        .filter(|(d, _)| d.is_error())
        .map(|(d, l)| (*l, d.code.as_str()))
        .collect()
}

#[test]
fn control_flow_labels() {
    let out = vm("class Main { function void main() { var int i, j;
        while (i < 10) { if (i = 5) { let j = -i; } else { let j = ~i; } }
        if (j) { let i = 1; }
        return; } }");
    let expected = "function Main.main 2
label WHILE_EXP0
push local 0
push constant 10
lt
not
if-goto WHILE_END0
push local 0
push constant 5
eq
if-goto IF_TRUE0
goto IF_FALSE0
label IF_TRUE0
push local 0
neg
pop local 1
goto IF_END0
label IF_FALSE0
push local 0
not
pop local 1
label IF_END0
goto WHILE_EXP0
label WHILE_END0
push local 1
if-goto IF_TRUE1
goto IF_FALSE1
label IF_TRUE1
push constant 1
pop local 0
label IF_FALSE1
push constant 0
return
";
    assert_eq!(out, expected);
}

#[test]
fn arrays_strings_and_constants() {
    let out = vm("class Main { function void main() {
        var Array a, b; var int i, j; var String s; var boolean t; var Array n;
        let a[i] = b[j]; let s = \"A\\\"\u{1F600}\"; let t = true; let n = null; return; } }");
    let expected = "function Main.main 7
push local 2
push local 0
add
push local 3
push local 1
add
pop pointer 1
push that 0
pop temp 0
pop pointer 1
push temp 0
pop that 0
push constant 4
call String.new 1
push constant 65
call String.appendChar 2
push constant 34
call String.appendChar 2
push constant -10179
call String.appendChar 2
push constant -8704
call String.appendChar 2
pop local 4
push constant 0
not
pop local 5
push constant 0
pop local 6
push constant 0
return
";
    assert_eq!(out, expected);
}

#[test]
fn objects_methods_and_constructors() {
    let out = vm("class Main { field int x; field Main next;
        constructor Main new(int ax) { let x = ax; return this; }
        method int get() { do next.get(); return x + get(); }
        function void f() { var int i; do i.foo(1); do Unknown.bar(); return; } }");
    let expected = "function Main.new 0
push constant 2
call Memory.alloc 1
pop pointer 0
push argument 0
pop this 0
push pointer 0
return
function Main.get 0
push argument 0
pop pointer 0
push this 1
call Main.get 1
pop temp 0
push this 0
push pointer 0
call Main.get 1
add
return
function Main.f 1
push local 0
push constant 1
call int.foo 2
pop temp 0
call Unknown.bar 0
pop temp 0
push constant 0
return
";
    assert_eq!(out, expected);
}

#[test]
fn duplicate_declarations_latest_wins() {
    let out = vm(
        "class Main { function void f(int x) { var int x, x; var int y;
        let x = 1; let y = 2; return; } }",
    );
    // Arguments are looked up before locals.
    assert!(out.starts_with(
        "function Main.f 3\npush constant 1\npop argument 0\npush constant 2\npop local 2\n"
    ));
}

#[test]
fn official_type_checks() {
    let m = |body: &str| {
        format!(
            "class Main {{ method void m() {{ var int vi; var char vc; var boolean vb; var String vs;\n{body}\nreturn; }} }}"
        )
    };
    // Reported one line above the `;`.
    assert_eq!(
        errors(&m("let vi = null;")),
        vec![(1, "jack-type-mismatch")]
    );
    assert_eq!(errors(&m("let vc = vi;")), vec![(1, "jack-type-mismatch")]);
    assert!(errors(&m("let vb = vi;")).is_empty());
    assert!(errors(&m("let vi = vs;")).is_empty());
    assert_eq!(
        errors(&m("let vi = vb + 1;")),
        vec![(2, "jack-illegal-operand"), (1, "jack-type-mismatch")]
    );
    assert_eq!(
        errors(&m("let vb = true & false;")),
        vec![(2, "jack-illegal-operand")]
    );
    assert!(errors(&m("let vb = true & vb;")).is_empty());
    assert_eq!(
        errors(&m("let vi = \"s\" + vi;")),
        vec![
            (2, "jack-string-cast"),
            (2, "jack-illegal-operand"),
            (1, "jack-type-mismatch")
        ]
    );
    assert!(errors(&m("let vi = (vb) + 1;")).is_empty());
    assert_eq!(
        errors(&m("let vi = vs[null + 1];")),
        vec![(2, "jack-illegal-operand")]
    );
    assert!(errors(&m("if (null) { } while (\"s\") { }")).is_empty());
}

#[test]
fn official_flow_checks() {
    let f = |body: &str| format!("class Main {{ method void m() {{ var int x;\n{body}\n}} }}");
    assert_eq!(errors(&f("")), vec![(3, "jack-missing-return")]);
    assert!(errors(&f("while (x) { return; }")).is_empty());
    assert!(errors(&f("return; while (x) { }")).is_empty());
    assert_eq!(
        errors(&f("if (x) { return; } else { }")),
        vec![(3, "jack-missing-return")]
    );
    assert_eq!(
        errors(&f("return;\nif (x) { let x = 1; }")),
        vec![(4, "jack-missing-return")]
    );
    let c = compile("Main", &f("return;\nlet x = 1;\nreturn;\nlet x = 2;"));
    let warnings: Vec<u32> = c
        .diagnostics
        .iter()
        .zip(&c.official_lines)
        .filter(|(d, _)| d.code.as_str() == "jack-unreachable-code")
        .map(|(_, l)| *l)
        .collect();
    assert_eq!(warnings, vec![3]);
    assert!(c.vm.is_some(), "a warning does not withhold the code");
}

#[test]
fn official_subroutine_checks() {
    assert_eq!(
        errors("class Main { function void f() { return 1; } }"),
        vec![(1, "jack-void-return-value")]
    );
    assert_eq!(
        errors("class Main { constructor Main new() { return; } }"),
        vec![
            (1, "jack-constructor-return"),
            (1, "jack-missing-return-value")
        ]
    );
    assert_eq!(
        errors("class Main { constructor int new() { return this; } }"),
        vec![(1, "jack-constructor-type")]
    );
    assert_eq!(
        errors("class Main { function void f() { return this; } }"),
        vec![(1, "jack-void-return-value"), (1, "jack-this-in-function")]
    );
    assert_eq!(
        errors("class Main { method void m() { return; } function void f() { do m(); return; } }"),
        vec![(1, "jack-method-from-function")]
    );
    assert_eq!(
        errors("class Main { field int x; function void f() { let x = 1; return; } }"),
        vec![(1, "jack-undefined-variable")]
    );
    assert_eq!(
        errors("class Main { function void f() { return; }\nfunction void f() { return; } }"),
        vec![(2, "jack-duplicate-subroutine")]
    );
    assert_eq!(
        errors("class Other { }"),
        vec![(1, "jack-class-name-mismatch")]
    );
    assert_eq!(
        errors("class Main { function int f() { return 32768; } }"),
        vec![(1, "jack-integer-too-big")]
    );
}

#[test]
fn own_class_call_checks_keep_vm_only_when_lenient() {
    let src =
        "class Main { function void f() { do Main.g(1); return; } function void g() { return; } }";
    let c = compile("Main", src);
    assert_eq!(c.official_lines, vec![1]);
    assert_eq!(c.diagnostics[0].code.as_str(), "jack-argument-count");
    assert!(c.vm.is_none());
    let lenient = n2t_jack::compile_with(
        "Main",
        src,
        &n2t_jack::Options {
            keep_vm_on_call_errors: true,
        },
    );
    assert!(lenient.vm.is_some());
}

#[test]
fn lexical_quirks() {
    // An unclosed string ends at the line break without an error.
    let c = compile(
        "Main",
        "class Main { function void f() { var String s; let s = \"abc\n; return; } }",
    );
    assert!(!c.has_errors());
    assert!(
        c.diagnostics
            .iter()
            .any(|d| d.code.as_str() == "jack-unclosed-string")
    );
    // A byte order mark is rejected, as by the official compiler.
    assert_eq!(
        errors("\u{feff}class Main { }")[0],
        (1, "jack-byte-order-mark")
    );
    // Fractions are truncated.
    assert!(vm("class Main { function int f() { return 1.9; } }").contains("push constant 1\n"));
    // Two consecutive line breaks inside a block comment count as one line
    // for the official compiler.
    assert_eq!(
        errors("class Main { function void f() { /*\n\n\n*/ let x = 1; return; } }"),
        vec![(3, "jack-undefined-variable")]
    );
    // An empty file.
    assert_eq!(errors(""), vec![(1, "jack-expected")]);
    // "Expected end-of-file" is reported on the line of the closing brace.
    assert_eq!(errors("class Main { }\n\n}"), vec![(1, "jack-expected")]);
}

#[test]
fn parse_recovers_from_half_typed_code() {
    let src = "class Main {
    field int count;
    method void a() {
        let count =
    }
    method void b(int x, ) {
        do Output.
    }
    function int c() {
        var int y
        if (y { return 1; }
        return y +;
    }
    method void d() { return; }
}";
    let p = parse(src);
    let class = p.class.as_ref().unwrap();
    let names: Vec<_> = class
        .subroutines
        .iter()
        .map(|s| s.name.as_ref().unwrap().name.as_str())
        .collect();
    assert_eq!(names, ["a", "b", "c", "d"]);
    assert!(!p.diagnostics.is_empty());
    // `do Output.` keeps a call with an empty name for completion.
    let b = &class.subroutines[1];
    match &b.statements[0] {
        Statement::Do { call: Some(c), .. } => {
            assert_eq!(c.receiver.as_ref().unwrap().name, "Output");
            assert_eq!(c.name.name, "");
        }
        other => panic!("{other:?}"),
    }
    // The `if` with a missing `)` still has its block.
    let c = &class.subroutines[2];
    assert!(matches!(c.statements[0], Statement::If { .. }));
    assert_eq!(c.locals[0].names[0].name, "y");
    // Comments and tokens.
    let p2 = parse("/** doc */ class Main { } // x");
    assert_eq!(p2.tokens[0].kind, TokenKind::DocComment);
    assert!(p2.class.unwrap().doc.is_some());
}

#[test]
fn symbols_resolve_every_identifier() {
    let src = "class Main {
    field int count;
    static Main inst;
    method void inc(int by) {
        var int t;
        let t = count + by;
        let count = t;
        do inst.inc(1);
        do Output.printInt(count);
        let missing = 1;
        return;
    }
}";
    let p = parse(src);
    let s = resolve(&p);
    let at = |needle: &str, nth: usize| {
        let off = src.match_indices(needle).nth(nth).unwrap().0;
        s.at(off).unwrap().clone()
    };
    match at("count", 1).role {
        Role::Variable { kind, ty, index } => {
            assert_eq!((kind, ty.as_str(), index), (VarKind::Field, "int", 0));
        }
        r => panic!("{r:?}"),
    }
    match at("by", 1).role {
        Role::Variable { kind, index, .. } => assert_eq!((kind, index), (VarKind::Argument, 1)),
        r => panic!("{r:?}"),
    }
    assert_eq!(at("Output", 0).role, Role::Class);
    assert_eq!(
        at("printInt", 0).role,
        Role::Subroutine {
            class: "Output".into(),
            kind: Some(n2t_jack::ast::SubroutineKind::Function)
        }
    );
    let inc_call = at("inc", 1);
    assert_eq!(inc_call.declaration, Some(at("inc", 0).span));
    assert_eq!(at("missing", 0).role, Role::Undefined);
    // References of the field: declaration plus three uses.
    let field = at("count", 0);
    assert!(field.is_declaration);
    assert_eq!(s.references(&field).count(), 4);
}

#[test]
fn program_level_checks() {
    let out = compile_program(&[
        (
            "Main",
            "class Main { function void main() { do Helper.go(1); do Math.max(1); return; } }",
        ),
        ("Helper", "class Helper { function void go() { return; } }"),
    ]);
    let main = &out.files[0].compiled;
    let codes: Vec<_> = main
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.code.as_str()))
        .collect();
    assert_eq!(
        codes,
        [
            (n2t_core::Severity::Error, "jack-argument-count"),
            (n2t_core::Severity::Warning, "jack-argument-count"),
        ]
    );
    assert!(out.diagnostics.is_empty());
    let none = compile_program(&[("A", "class A { }")]);
    assert_eq!(none.diagnostics.len(), 1);
}

#[test]
fn os_api_table() {
    assert_eq!(os::OS_CLASSES.len(), 8);
    let f = os::find("Screen", "drawLine").unwrap();
    assert_eq!(f.params.len(), 4);
    assert_eq!(f.return_type, "void");
    assert!(os::find("Memory", "alloc").is_some());
    assert!(os::api().iter().all(|s| !s.description.is_empty()));
}

#[test]
fn deep_term_shape() {
    let p = parse("class Main { function int f() { return -(1); } }");
    let sub = &p.class.unwrap().subroutines[0];
    match &sub.statements[0] {
        Statement::Return { value: Some(e), .. } => assert!(matches!(e.first, Term::Unary { .. })),
        other => panic!("{other:?}"),
    }
}
