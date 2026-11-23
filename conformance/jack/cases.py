"""Generated corpus for run.py.

Every entry is the complete text of one original Jack file; `CLS` is replaced by
the class (and file) name. The corpus mixes valid programs (whose .vm output is
byte-compared) and invalid ones (accept/reject and first error line compared).
"""

import itertools

CASES = []


def add(src):
    CASES.append(src)


# A body with one variable of each kind, used by many statement probes.
DECLS = ("var int vi; var char vc; var boolean vb; var String vs; var Array va; var Foo vf; var CLS vq;")
HELPERS = (
    " function char fc() { return 0; } function void ti(int a) { return; }"
    " function int ri() { return 0; } method void mm(int a, int b) { return; }"
    " constructor CLS new() { return this; } }\n"
)


def stmt(body, kind="method", ret="void"):
    add("class CLS { %s %s m() { %s\n%s\n return; }\n%s" % (kind, ret, DECLS, body, HELPERS))


# --- the operand type matrix -------------------------------------------------
TERMS = ["1", '"s"', "true", "null", "this", "vi", "vc", "vb", "vs", "CLS.fc()", "va[1]", "(vb)", "(vi)", "-vb", "~vi"]
for a, b in itertools.product(TERMS, TERMS):
    for target in ("vi", "vc", "vb"):
        stmt("let %s = %s + %s;" % (target, a, b))
for t in TERMS:
    stmt("let vs = %s;" % t)
    stmt("let vi = va[%s];" % t)
    stmt("let va[%s] = %s;" % (t, t))
for e in ["vi < vi", "vb = vb", "vb & vb", "vi & vi", "vc + 1", "65 + 1", "1 + vc", "vb | true", "true & false",
          "vi + -1", "vb + -1", "va[vb + 1]", "va[null + 1]", 'va["s" + vi]', "va[true + vb]",
          '"s" * vi', '"s" = vs', '"s" + vs.length()', '-"s" + vi', '"s" + ~vb', "vs + vb", "vf + vb"]:
    for target in ("vi", "vc", "vb", "vs"):
        stmt("let %s = %s;" % (target, e))
for e in ["vb + 1", '"s" + 1', "null"]:
    stmt("do CLS.ti(%s);" % e)
    stmt("if (%s) { }" % e)
    stmt("while (%s) { }" % e)
    stmt("return %s;" % e)

# --- error lines: the same statements spread over lines ----------------------
for s in ["let vi = null ;", "let vi = vb + 1 ;", 'let vi = "s" + vi ;', "let vi = va [ null ] ;", "let x = 1 ;",
          "let vi = vi + - vb ;", 'let vi = "s" + vs . length ( ) ;', "do CLS . ti ( 1 , 2 ) ;", "do CLS . nothere ( 1 ) ;",
          "do CLS . mm ( 1 , 2 ) ;", "do vq . new ( ) ;", "let vi = 32768 ;"]:
    toks = s.split()
    for mask in (0, 1, 5, 10, (1 << len(toks)) - 1, 0b0101010101, 0b1100110011):
        out = ""
        for i, t in enumerate(toks):
            out += t + ("\n" if (mask >> i) & 1 else " ")
        stmt(out)

# --- statements and flow -----------------------------------------------------
FLOW = [
    "", "return;", "return; return;", "return; let x = 1;", "if (x) { return; }", "if (x) { return; } else { return; }",
    "if (x) { } else { return; }", "if (x) { return; } else { }", "while (x) { return; }", "while (true) { }",
    "return; let x = 1; return; let x = 2;", "return; while (x) { }", "return; if (x) { } else { }",
    "if (x) { return; } else { return; } let x = 1;", "while (x) {\nreturn;\nlet x = 1;\n}\nreturn;",
    "return;\nif (x) {\nreturn;\nlet x = 1;\n}", "if (x) {\nreturn;\nlet x = 1;\n}\nif (x) {\nreturn;\nlet x = 1;\n}\nreturn;",
    "return;\nwhile (x) {\nif (x) {\n}\nreturn;\nlet x = 1;\n}", "return;\nwhile (x) {\nif (x) {\n}\n}",
    "if (x) {\nlet x = 1;\n}\nelse {\nreturn;\nlet x = 2;\n}", "return;\nlet x = 0;\nif (x) {\nreturn;\n}\nlet x = 1;\nreturn;\nlet x = 2;",
    "while (x) {\nreturn;\n}\nwhile (x) {\nreturn;\n}\nreturn;", "return;\nif (x) {\nlet x = 1;\n}\nelse { return; }",
]
for body in FLOW:
    for kind, ret, val in (("method", "void", "return;"), ("function", "int", "return 1;")):
        add("class CLS { %s %s m() { var int x;\n%s\n} }\n" % (kind, ret, body.replace("return;", val)))

# --- subroutine-level checks -------------------------------------------------
SUBS = [
    "function void f() { return 1; }", "function int f() { return; }", "constructor CLS new() { return 1; }",
    "constructor CLS new() { return; }", "constructor int new() { return this; }", "constructor Foo new() { return this; }",
    "constructor CLS new() { var CLS c; return c; }", "constructor CLS new() { return (this); }",
    "constructor CLS new() { return this + 1; }", "constructor CLS new() { return -this; }",
    "method int f() { return this; }", "function void f() { var CLS c; let c = this; return; }",
    "function void f() { return this; }", "function void f() { return; } function void f() { return; }",
    "function int f() { return true; }", "function boolean f() { return null; }",
    "field int x; function void f() { let x = 1; return; }", "field int x; function void f() { var int y; let y = x[1]; return; }",
    "field CLS fld; function void f() { do fld.bar(); return; }", "static CLS s; function void f() { do s.bar(); return; }",
    "method void m() { return; } function void g() { do m(); return; }", "method void m() { return; } function void g() { do CLS.m(); return; }",
    "function void h() { return; } method void g() { do h(); return; }", "method void g() { do nothere(); return; }",
    "function void g() { do nothere(); return; }", "method void g() { var CLS c; do c.nothere(); return; }",
    "method void g() { var Foo c; do c.nothere(); return; }", "method void g() { do CLS.nothere(); return; }",
    "method void g() { do g(1); return; }", "method void g(int a) { var CLS c; do c.g(); return; }",
    "method void g() { do Output.printInt(); do Output.nothere(); do Math.multiply(1, 2); return; }",
    "method void g() { var int s; do s.nothere(); return; }", "method void g() { do this.g(); return; }",
    "constructor CLS new() { do g(); return this; } method void g() { return; }",
    "constructor CLS new() { return this; } method void g() { do new(); return; }",
    "function void g() { do CLS.nothere(); let x = 1; return; }",
    "field int x, y; static int x; method void f() { let x = 1; let y = x; return; }",
    "function void f(int x) { var int x; let x = 1; return; }", "function void f(int a, int a) { let a = 1; return; }",
    "function void f() { var int x, x; var int y; let x = 1; let y = 2; return; }",
    "field int a, b; field CLS a; constructor CLS new() { let a = 1; return this; }",
    "function void f() { var int i; do i.foo(1); do undefinedthing.bar(); return; }",
    "function void f() { var int f; let f = 1; do CLS.f(); return; }", "static int s; function void f() { var CLS m; let s = m; let m = CLS; return; }",
    "method void m() { var int y; let y = m; return; }", "method void m() { var int y; let y = CLS.m; return; }",
]
for s in SUBS:
    add("class CLS {\n%s\n}\n" % s.replace(" { ", " {\n ").replace("; ", ";\n "))

# --- syntax errors -------------------------------------------------------------
SYNTAX = [
    "class CLS { function void f() { return; }", "class CLS { function void f() { return; } } extra",
    "class CLS { function void f() { return } }", "class CLS { function void f() { var int x; let x = ; return; } }",
    "class CLS { function void f() { var int x; let x = 1 return; } }", "class CLS { function void f() { var int x; let x 1; return; } }",
    "class CLS { function void f() { var int x; x = 1; return; } }", "class CLS { function void f() { var int x; do x; return; } }",
    "class CLS { function void f() { var int x; do 1; return; } }", "class CLS { function void f() { var int x; if x { } return; } }",
    "class CLS { function void f() { var int x; if (x) return; } }", "class CLS { function void f() { var int x; while (x) { } else { } return; } }",
    "class CLS { function void f() { var int x; let x = 1 + ; return; } }", "class CLS { function void f() { var int x; let x = (1 + 2; return; } }",
    "class CLS { function void f() { var Foo x; do x.y.z(); return; } }", "class CLS { function void f() { do f; return; } }",
    "class CLS { function void f() { do f()(); return; } }", "class CLS { function void f() { var int 1x; return; } }",
    "class CLS { function void f() { var int class; return; } }", "class CLS { function void f() { var void x; return; } }",
    "class CLS { field void x; function void f() { return; } }", "class CLS { function void f(void x) { return; } }",
    "class CLS { function void f() { return; } field int x; }", "class CLS { function void f() { let x = 1; var int x; return; } }",
    "class CLS { function void f() { return; } }}", "class CLS { }", "class CLS", "class", "", "// only a comment\n", "/* only */",
    "class CLS {\nmethod void m ( int x int y ) { return ; } }", "class CLS {\nmethod void m ( ) { var int ; return ; } }",
    "class\nCLS { method void m ( ) { return ; } } }", "class CLS {\nmethod void m ( ) { return ; }\n",
    "class Other { function void f() { return; } }", "Class CLS { }", "class CLS { function void f() { var int x; let x[1 = 2; return; } }",
    "class CLS { function void f() { do Output.printInt(1 2); return; } }", "class CLS { function void f() { do Output.printInt(,); return; } }",
    "class CLS { function void f() { var int x; let x = x.; return; } }", "class CLS { function void f() { var int x; let x = int; return; } }",
    "class CLS { function void f() { else { } return; } }", "class CLS { function f() { return; } }", "class CLS { function void () { return; } }",
    "class CLS { function void f { return; } }", "class CLS { function void f() return; }", "class CLS { void f() { return; } }",
    "class CLS { function void f() { var int x; let this = 1; return; } }", "class CLS { function void f() { var int x; let x = 1; } x }",
]
for s in SYNTAX:
    add(s)

# --- lexical edge cases --------------------------------------------------------
LEX = [
    'let vs = "a\\\\b";', 'let vs = "a\\nb";', 'let vs = "a\\tb\\q";', 'let vs = "a\\";', 'let vs = "a\nb";', 'let vs = "abc',
    'let vs = "";', 'let vs = "é€";', 'let vs = "a//b/*c";', "let vs = \"it's\";", "let vi = 007;", "let vi = 32767;",
    "let vi = 32768;", "let vi = 99999999999999999999;", "let vi = 0x10;", "let vi = 1; /* unterminated", "let vi = 1; /** api */ let vi = 2;",
    "let vi = 1; /* a /* b */ let vi = 2;", "let vi = 1; */", "let vi = 1; // é comment", "let vi = 1; /* é € */",
    "let vi = #;", "let vi = 1; $", "let vi = 1; @", "let vi = 1; ?", "let vi = 1; \\", "let vi = 1; `", "let vi = 'a';",
    "let vi = 1; é", "let vi_2 = 1;", "let vi = vi-1;", "let vi = -1;", "let vi = 1.5;", "let vi = 1.;", "let vi = .5;",
    "let vi = 1e5;", "let vi = 3x;", "let vi = 1.2.3;", "let vi = va.length();", "let vi = vi - .5;", "let vi = --1;", "let vi = 1-;",
    'let vs = "\\101";', 'let vs = "\\0";', 'let vs = "\\r\\b\\f\\a\\v";', 'let vs = "\\u0041";', "let vs = 'abc';",
    'let vs = "a\x01b";', "let vi = 1;\r let vi = x;", "let vi = 1;\r\n\r\n let vi = x;", "let vi = 1;\n\r let vi = x;",
    "let vi = 1; // c\r let vi = x;", "let vi = 1; /* \r\n \n */ let vi = x;", "let vi = 1; /*\n\n\n*/ let vi = x;",
    "let vi = 1; /*\r\n\r\n*/ let vi = x;", "let vi\u0085 = 1;", "let vi$ = 1;", "let vi\x7f = 1;", "let vi  = 1;",
    'let vs = "\U0001F600";', 'let vs = "\\477";', "Let vi = 1;", "let vi = True;", "let vi = 32767.5;", "let vi = 00032768;",
    "let vi = 3.0;", 'let vs = "a\\\nb";\nlet vi = x;', 'let vs = "tab\there";', "let vi = 2/*c*/+3;", "let vi = 2//c\n+3;",
]
for s in LEX:
    add("class CLS { function void f() { %s\n%s\n return; } }\n" % (DECLS.replace(" var CLS vq;", ""), s))
for s in ["﻿class CLS { }\n", "class CLS { }\r\n", "class CLS {\r\n function void f() {\r\n\treturn;\r\n }\r\n}\r\n",
          "class CLS { function void f() { return; } } // trailing", "class CLS { function void f() { return; } } /* trailing",
          "class CLS { function void f() { return; } }\x00", "class CLS { function void f() { var int i; let i = 1\x01; return; } }",
          "class CLS { function void f() { return; } } /* é\n\n", "\n\n\nclass CLS { function void f() { return } }\n\n\n",
          "class CLS { function void f() { return; } } /* x\n\n\n\n", "class CLS { function void f() { return; } }\n/*\n\n\n*/\n}"]:
    add(s)

# --- deep nesting (accepted by the official compiler up to at least 10000) -----
for d in (50, 99, 100, 390):
    add("class CLS { function int f() { var int x; let x = %s1%s; return x; } }\n" % ("(" * d, ")" * d))
for d in (60, 126, 200):
    add("class CLS { function int f() { var int x; let x = %s1; return x; } }\n" % ("-" * d))
    add("class CLS { function int f() { var int x; %s let x = 1; %s return x; } }\n" % ("if (x) {" * d, "}" * d))
add("class CLS { function int f() { var int x; let x = 1%s; return x; } }\n" % ("+1" * 5000))
add("class CLS { function int f(int y) { var int x; let x = %s1%s; return x; } }\n" % ("CLS.f(" * 99, ")" * 99))
add("class CLS { function int f(int y) { var int x; let x = %s1%s; return x; } }\n" % ("CLS.f(" * 100, ")" * 100))
add("class CLS { function int f() { var Array a; let a[%s1%s] = a[%s2%s]; return 0; } }\n" % ("(" * 99, ")" * 99, "(" * 98, ")" * 98))
