//! Line-level syntax tree for Hack assembly.
//!
//! Every line is parsed on its own, so a syntax error affects only its line.
//! The accept/reject decision for each line matches the official assembler
//! (see the module documentation of [`crate::asm`]).

use n2t_core::{Diagnostic, DiagnosticCode, Span};

use super::lexer::{self, TokKind, Token};
use crate::isa;

/// The parsed lines of a source file, with syntax diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAsm {
    pub lines: Vec<AsmLine>,
    pub diagnostics: Vec<Diagnostic>,
}

/// One physical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsmLine {
    /// The line without its terminator.
    pub span: Span,
    /// `None` for a blank or comment-only line.
    pub item: Option<Item>,
    /// The `//` comment, from the first `/` to the end of the line.
    pub comment: Option<Span>,
    /// Whether this line has a syntax error.
    pub has_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    A(AInstruction),
    C(CInstruction),
    Label(LabelDef),
}

impl Item {
    /// The span of the whole item (without comment).
    pub fn span(&self) -> Span {
        match self {
            Item::A(a) => a.span,
            Item::C(c) => c.span,
            Item::Label(l) => l.span,
        }
    }

    /// Whether this item occupies a ROM word (A- and C-instructions do, labels
    /// do not). Lines with syntax errors count too, so that addresses after
    /// them stay meaningful in the editor.
    pub fn is_instruction(&self) -> bool {
        !matches!(self, Item::Label(_))
    }
}

/// A name with its location. The span runs from the first to the last
/// character of the name in the source; `text` has any spaces and tabs inside
/// it removed (the official assembler ignores them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

/// `@value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AInstruction {
    pub span: Span,
    /// The `@`.
    pub at: Span,
    /// `None` if missing (a syntax error).
    pub operand: Option<Operand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    /// A decimal constant in -32768..=32767. The emitted word is `value as u16`;
    /// negative values set the top bit (see `asm-negative-constant`).
    Number { value: i16, span: Span },
    /// Anything else, including text such as `40000` or `1abc` that the
    /// official assembler treats as a symbol.
    Symbol(Name),
}

impl Operand {
    pub fn span(&self) -> Span {
        match self {
            Operand::Number { span, .. } => *span,
            Operand::Symbol(n) => n.span,
        }
    }
}

/// A mnemonic field of a C-instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub text: String,
    pub span: Span,
    /// The 3-bit dest/jump code or 7-bit comp code; `None` if the mnemonic is invalid.
    pub code: Option<u8>,
}

/// `dest=comp;jump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CInstruction {
    pub span: Span,
    /// `None` when there is no `dest=` part (or it is written as just `=`).
    pub dest: Option<Field>,
    /// The `=`, if present.
    pub equals: Option<Span>,
    /// `None` if missing (a syntax error).
    pub comp: Option<Field>,
    /// The `;`, if present.
    pub semicolon: Option<Span>,
    /// `None` when there is no jump (or it is written as just `;`).
    pub jump: Option<Field>,
}

impl CInstruction {
    /// The machine word, if every present field is valid.
    pub fn encode(&self) -> Option<u16> {
        let comp = self.comp.as_ref()?.code?;
        let dest = match &self.dest {
            Some(f) => f.code?,
            None => 0,
        };
        let jump = match &self.jump {
            Some(f) => f.code?,
            None => 0,
        };
        Some(isa::encode_c(comp, dest, jump))
    }
}

/// `(NAME)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDef {
    pub span: Span,
    pub open: Span,
    /// `None` if missing (a syntax error).
    pub name: Option<Name>,
    pub close: Option<Span>,
}

fn code(c: &'static str) -> DiagnosticCode {
    DiagnosticCode::Other(c)
}

/// Parses Hack assembly into a line-level syntax tree. Never panics.
pub fn parse(source: &str) -> ParsedAsm {
    let mut lines = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, span) in lexer::split_lines(source).into_iter().enumerate() {
        let lexed = lexer::lex_line(source, span);
        let mut line_diags = Vec::new();
        let item = parse_tokens(&lexed.tokens, &mut line_diags);
        if index == 0 && source.starts_with('\u{feff}') && !line_diags.is_empty() {
            line_diags.clear();
            line_diags.push(Diagnostic::error(
                code("asm-byte-order-mark"),
                "the file starts with a UTF-8 byte order mark, which the official assembler \
                 rejects; save the file as UTF-8 without BOM",
                Span::new(0, 3),
            ));
        }
        let has_error = line_diags.iter().any(Diagnostic::is_error);
        diagnostics.append(&mut line_diags);
        lines.push(AsmLine {
            span,
            item,
            comment: lexed.comment,
            has_error,
        });
    }
    ParsedAsm { lines, diagnostics }
}

fn after(t: &Token) -> Span {
    Span::empty(t.span.end)
}

fn trailing(tokens: &[Token], diags: &mut Vec<Diagnostic>, what: &str) {
    if let (Some(first), Some(last)) = (tokens.first(), tokens.last()) {
        diags.push(Diagnostic::error(
            code("asm-trailing-text"),
            format!(
                "unexpected `{}` after {what}; only a `//` comment may follow on the same line",
                first.text
            ),
            first.span.merge(last.span),
        ));
    }
}

fn parse_tokens(tokens: &[Token], diags: &mut Vec<Diagnostic>) -> Option<Item> {
    let first = tokens.first()?;
    let span = first.span.merge(tokens[tokens.len() - 1].span);
    if first.is_char('@') {
        return Some(Item::A(parse_a(tokens, span, diags)));
    }
    if first.is_char('(') {
        return Some(Item::Label(parse_label(tokens, span, diags)));
    }
    Some(Item::C(parse_c(tokens, span, diags)))
}

fn parse_a(tokens: &[Token], span: Span, diags: &mut Vec<Diagnostic>) -> AInstruction {
    let at = tokens[0].span;
    let Some(value) = tokens.get(1) else {
        diags.push(Diagnostic::error(
            code("asm-missing-operand"),
            "`@` must be followed by a number or a symbol, e.g. `@17` or `@LOOP`",
            at,
        ));
        return AInstruction {
            span,
            at,
            operand: None,
        };
    };
    trailing(&tokens[2..], diags, &format!("`@{}`", value.text));
    let operand = match lexer::parse_number(&value.text) {
        Some(n) if value.kind == TokKind::Word => {
            if n < 0 {
                diags.push(Diagnostic::warning(
                    code("asm-negative-constant"),
                    format!(
                        "`@{}` assembles to {:016b}: the top bit is set, so the CPU executes \
                         this word as a C-instruction, not as a constant. The official assembler \
                         accepts it, but to load a negative number write e.g. `@{}` then `A=-A`",
                        value.text,
                        n as u16,
                        i32::from(n).unsigned_abs()
                    ),
                    value.span,
                ));
            }
            Operand::Number {
                value: n,
                span: value.span,
            }
        }
        _ => {
            symbol_warnings(value, diags, true);
            Operand::Symbol(Name {
                text: value.text.clone(),
                span: value.span,
            })
        }
    };
    AInstruction {
        span,
        at,
        operand: Some(operand),
    }
}

/// Warnings for symbol names that the official assembler accepts but that the
/// book's rules (letters, digits, `_ . $ :`, not starting with a digit) do not.
fn symbol_warnings(t: &Token, diags: &mut Vec<Diagnostic>, in_a_instruction: bool) {
    let text = &t.text;
    let starts_with_digit = text.chars().next().is_some_and(|c| c.is_ascii_digit());
    let looks_numeric = text
        .trim_start_matches(['-', '+'])
        .chars()
        .all(|c| c.is_ascii_digit())
        && text.chars().any(|c| c.is_ascii_digit());
    if in_a_instruction && looks_numeric {
        diags.push(Diagnostic::warning(
            code("asm-number-out-of-range"),
            format!(
                "`{text}` does not fit in an A-instruction (the range is 0..32767), so the \
                 official assembler treats it as a variable named `{text}`. Use a value in range"
            ),
            t.span,
        ));
        return;
    }
    let bad = text
        .chars()
        .find(|&c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | ':')));
    if let Some(c) = bad {
        diags.push(Diagnostic::warning(
            code("asm-nonportable-symbol"),
            format!(
                "symbol `{text}` contains `{}`; the official assembler accepts it, but symbols \
                 should use only letters, digits, `_`, `.`, `$` and `:`",
                c.escape_debug()
            ),
            t.span,
        ));
    } else if starts_with_digit {
        diags.push(Diagnostic::warning(
            code("asm-nonportable-symbol"),
            format!(
                "symbol `{text}` starts with a digit; the official assembler accepts it, but \
                 symbols should not start with a digit"
            ),
            t.span,
        ));
    }
}

fn parse_label(tokens: &[Token], span: Span, diags: &mut Vec<Diagnostic>) -> LabelDef {
    let open = tokens[0].span;
    let mut label = LabelDef {
        span,
        open,
        name: None,
        close: None,
    };
    let Some(name) = tokens.get(1) else {
        diags.push(Diagnostic::error(
            code("asm-missing-label-name"),
            "expected a label name after `(`, e.g. `(LOOP)`",
            Span::empty(open.end),
        ));
        return label;
    };
    label.name = Some(Name {
        text: name.text.clone(),
        span: name.span,
    });
    match tokens.get(2) {
        Some(t) if t.is_char(')') => {
            label.close = Some(t.span);
            if lexer::parse_number(&name.text).is_none() {
                symbol_warnings(name, diags, false);
            }
            trailing(&tokens[3..], diags, &format!("label `({})`", name.text));
        }
        Some(t) => diags.push(Diagnostic::error(
            code("asm-unclosed-label"),
            format!(
                "expected `)` after the label name `{}`, found `{}`; a label name may only \
                 contain letters, digits and `_ . $ :`",
                name.text, t.text
            ),
            t.span,
        )),
        None => diags.push(Diagnostic::error(
            code("asm-unclosed-label"),
            format!("expected `)` to close the label `({}`", name.text),
            after(name),
        )),
    }
    label
}

fn field(t: &Token, code: Option<u8>) -> Field {
    Field {
        text: t.text.clone(),
        span: t.span,
        code: if t.kind == TokKind::Word { code } else { None },
    }
}

fn parse_c(tokens: &[Token], span: Span, diags: &mut Vec<Diagnostic>) -> CInstruction {
    let mut c = CInstruction {
        span,
        dest: None,
        equals: None,
        comp: None,
        semicolon: None,
        jump: None,
    };
    let mut i = 0;
    if tokens[0].is_char('=') {
        c.equals = Some(tokens[0].span);
        i = 1;
    } else if tokens.get(1).is_some_and(|t| t.is_char('=')) {
        let d = field(&tokens[0], isa::dest_code(&tokens[0].text));
        if d.code.is_none() {
            diags.push(Diagnostic::error(
                code("asm-invalid-dest"),
                dest_message(&d.text),
                d.span,
            ));
            c.dest = Some(d);
            c.equals = Some(tokens[1].span);
            // Java stops at the first error on a line; so do we.
            c.comp = tokens.get(2).map(|t| field(t, isa::comp_code(&t.text)));
            return c;
        }
        c.dest = Some(d);
        c.equals = Some(tokens[1].span);
        i = 2;
    }
    let Some(comp_tok) = tokens.get(i) else {
        let at = tokens[i - 1].span;
        diags.push(Diagnostic::error(
            code("asm-missing-comp"),
            "expected a computation after `=`, e.g. `D=M` or `M=D+1`",
            Span::empty(at.end),
        ));
        return c;
    };
    let comp = field(comp_tok, isa::comp_code(&comp_tok.text));
    if comp.code.is_none() {
        let msg = comp_message(
            comp_tok,
            c.dest.is_none() && c.equals.is_none(),
            tokens.len() == 1,
        );
        diags.push(Diagnostic::error(code("asm-invalid-comp"), msg, comp.span));
        c.comp = Some(comp);
        return c;
    }
    c.comp = Some(comp);
    i += 1;
    let Some(sep) = tokens.get(i) else {
        return c;
    };
    if !sep.is_char(';') {
        diags.push(Diagnostic::error(
            code("asm-expected-semicolon"),
            format!(
                "expected `;` and a jump (e.g. `;JGT`) or the end of the line after the \
                 computation, found `{}`",
                sep.text
            ),
            sep.span,
        ));
        return c;
    }
    c.semicolon = Some(sep.span);
    i += 1;
    let Some(jump_tok) = tokens.get(i) else {
        return c;
    };
    let jump = field(jump_tok, isa::jump_code(&jump_tok.text));
    if jump.code.is_none() {
        diags.push(Diagnostic::error(
            code("asm-invalid-jump"),
            jump_message(&jump.text),
            jump.span,
        ));
        c.jump = Some(jump);
        return c;
    }
    c.jump = Some(jump);
    trailing(&tokens[i + 1..], diags, "the jump");
    c
}

fn dest_message(text: &str) -> String {
    let upper = text.to_ascii_uppercase();
    let mut letters: Vec<char> = upper.chars().collect();
    letters.sort_by_key(|c| match c {
        'A' => 0,
        'M' => 1,
        'D' => 2,
        _ => 3,
    });
    let reordered: String = letters.into_iter().collect();
    let hint = if isa::dest_code(&upper).is_some() && upper != text {
        format!("; mnemonics are upper case: write `{upper}=`")
    } else if isa::dest_code(&reordered).is_some() {
        format!("; the letters must be in the order A, M, D: write `{reordered}=`")
    } else {
        String::new()
    };
    format!("`{text}` is not a destination; use one of M, D, MD, A, AM, AD, AMD{hint}")
}

fn jump_message(text: &str) -> String {
    let upper = text.to_ascii_uppercase();
    let hint = if isa::jump_code(&upper).is_some() {
        format!("; mnemonics are upper case: write `{upper}`")
    } else {
        String::new()
    };
    format!("`{text}` is not a jump; use one of JGT, JEQ, JGE, JLT, JNE, JLE, JMP{hint}")
}

fn comp_message(t: &Token, bare: bool, alone: bool) -> String {
    let text = &t.text;
    if text.is_empty() || t.kind != TokKind::Word {
        if t.is_char(';') {
            return "expected a computation before `;`, e.g. `0;JMP` or `D;JGT`".to_string();
        }
        return format!(
            "`{text}` is not an instruction: a line must be `@value`, `(LABEL)` or \
             `dest=comp;jump`"
        );
    }
    let upper = text.to_ascii_uppercase();
    if isa::comp_code(&upper).is_some() {
        return format!("`{text}` is not a computation; mnemonics are upper case: write `{upper}`");
    }
    for op in ['+', '&', '|'] {
        if let Some((l, r)) = text.split_once(op) {
            let swapped = format!("{r}{op}{l}");
            if let Some(c) =
                isa::comp_code(&swapped).or_else(|| isa::comp_code(&swapped.to_ascii_uppercase()))
            {
                let canonical = isa::comp_mnemonic(c).unwrap_or("");
                return format!("`{text}` is not a computation; write `{canonical}`");
            }
        }
    }
    if bare
        && alone
        && text
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
    {
        return format!(
            "`{text}` is not an instruction. To define a label write `({text})`; to load an \
             address write `@{text}`"
        );
    }
    let all: Vec<&str> = isa::COMPS.iter().map(|&(m, _)| m).collect();
    format!(
        "`{text}` is not a computation the Hack ALU can do in one instruction; valid \
         computations are {}",
        all.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(src: &str) -> (Option<Item>, Vec<Diagnostic>) {
        let p = parse(src);
        (
            p.lines.into_iter().next().and_then(|l| l.item),
            p.diagnostics,
        )
    }

    fn errors(src: &str) -> Vec<&'static str> {
        parse(src)
            .diagnostics
            .into_iter()
            .filter(|d| d.is_error())
            .map(|d| d.code.as_str())
            .collect()
    }

    #[test]
    fn c_instruction_fields_have_spans() {
        let (item, diags) = item("  AM = M+1 ; JGT // hi");
        assert!(diags.is_empty());
        let Some(Item::C(c)) = item else { panic!() };
        assert_eq!(c.dest.as_ref().unwrap().span, Span::new(2, 4));
        assert_eq!(c.comp.as_ref().unwrap().span, Span::new(7, 10));
        assert_eq!(c.jump.as_ref().unwrap().span, Span::new(13, 16));
        assert_eq!(c.encode(), Some(0b1111110111101001));
    }

    #[test]
    fn accepts_what_java_accepts() {
        for ok in [
            "D=M;",
            "=M",
            "=M;",
            "D;",
            "M",
            "0",
            "D = M",
            "D=A&D",
            "M=M+D",
            "(;)",
            "(()",
            "(é)",
            "@é",
            "@;",
            "@(",
            "@-1",
            "@32768",
            "@1abc",
            "(1L)",
            "(a+b)",
            "@a//b",
            "@a/ /b",
            "( L )",
            "AMD=M",
            "D=M;JMP//c",
        ] {
            assert_eq!(errors(ok), Vec::<&str>::new(), "{ok:?}");
        }
    }

    #[test]
    fn rejects_what_java_rejects() {
        for bad in [
            "d=a",
            "DM=M",
            "null=M",
            "D=1+M",
            "D=M+0",
            ";JMP",
            "0;jmp",
            "D;JMP;",
            "D==M",
            "D=M=A",
            "D=",
            "=",
            ";",
            "@",
            "(L",
            "L)",
            "()",
            "(L)D=M",
            "((L)",
            "(a;b)",
            "(L//)",
            "@a;JMP",
            "@a=b",
            "@1/2",
            "@aéb",
            "@1\u{c}",
            "/ c",
            "/* c */",
            "\u{feff}@1",
            "D=M;null",
            "JMP",
            "@x\0",
        ] {
            assert!(!errors(bad).is_empty(), "{bad:?}");
        }
    }

    #[test]
    fn bom_gets_a_specific_message() {
        assert_eq!(errors("\u{feff}// c\n@1\n"), ["asm-byte-order-mark"]);
    }

    #[test]
    fn helpful_messages() {
        let msg = |s: &str| parse(s).diagnostics[0].message.clone();
        assert!(msg("D=1+M").contains("`M+1`"));
        assert!(msg("d=M").contains("`D=`"));
        assert!(msg("DM=M").contains("`MD=`"));
        assert!(msg("0;jmp").contains("`JMP`"));
        assert!(msg("LOOP").contains("(LOOP)"));
    }
}
