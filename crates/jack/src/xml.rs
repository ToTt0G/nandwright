//! The XML output of the project-10 syntax analyzer, so students can compare
//! their own analyzer's output with a reference inside the editor.
//!
//! Lines end with `\n`; the course's reference files use `\r\n`, so compare
//! modulo line endings (as the course's TextComparer does).

use n2t_core::Diagnostic;

use crate::lexer::{self, Keyword, Token, TokenKind};

/// `<tokens>` XML (the `*T.xml` format). Fails with the lexical/syntax
/// diagnostics if the file does not parse.
pub fn tokens_xml(source: &str) -> Result<String, Vec<Diagnostic>> {
    let toks = checked_tokens(source)?;
    let mut out = String::from("<tokens>\n");
    for t in &toks {
        out.push_str(&leaf(source, t));
        out.push('\n');
    }
    out.push_str("</tokens>\n");
    Ok(out)
}

/// Parse-tree XML (the `*.xml` format). Fails with the lexical/syntax
/// diagnostics if the file does not parse.
pub fn parse_xml(source: &str) -> Result<String, Vec<Diagnostic>> {
    let toks = checked_tokens(source)?;
    let mut w = Writer {
        src: source,
        toks: &toks,
        pos: 0,
        indent: 0,
        out: String::new(),
    };
    match w.class() {
        Some(()) => Ok(w.out),
        None => Err(Vec::new()),
    }
}

fn checked_tokens(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let parsed = crate::parse(source);
    let errors: Vec<Diagnostic> = parsed
        .diagnostics
        .into_iter()
        .filter(|d| d.is_error())
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(parsed
        .tokens
        .into_iter()
        .filter(|t| !t.kind.is_trivia())
        .collect())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn leaf(src: &str, t: &Token) -> String {
    let text = t.text(src);
    let (tag, value) = match t.kind {
        TokenKind::Keyword(_) => ("keyword", text.to_string()),
        TokenKind::Symbol(_) => ("symbol", escape(text)),
        TokenKind::Identifier => ("identifier", text.to_string()),
        TokenKind::IntConst => ("integerConstant", text.to_string()),
        TokenKind::StringConst => {
            let units = lexer::string_units(text);
            ("stringConstant", escape(&String::from_utf16_lossy(&units)))
        }
        _ => ("unknown", escape(text)),
    };
    format!("<{tag}> {value} </{tag}>")
}

struct Writer<'a> {
    src: &'a str,
    toks: &'a [Token],
    pos: usize,
    indent: usize,
    out: String,
}

impl Writer<'_> {
    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }
    fn at_sym(&self, c: u8) -> bool {
        self.peek().is_some_and(|t| t.is_symbol(c))
    }
    fn at_kw(&self, k: &[Keyword]) -> bool {
        self.peek()
            .is_some_and(|t| matches!(t.kind, TokenKind::Keyword(kw) if k.contains(&kw)))
    }
    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }
    fn open(&mut self, tag: &str) {
        self.line(&format!("<{tag}>"));
        self.indent += 1;
    }
    fn close(&mut self, tag: &str) {
        self.indent = self.indent.saturating_sub(1);
        self.line(&format!("</{tag}>"));
    }
    /// Emits the current token.
    fn tok(&mut self) -> Option<()> {
        let t = *self.peek()?;
        self.pos += 1;
        let l = leaf(self.src, &t);
        self.line(&l);
        Some(())
    }
    /// Emits tokens up to and including the symbol `c`.
    fn through(&mut self, c: u8) -> Option<()> {
        while !self.at_sym(c) {
            self.tok()?;
        }
        self.tok()
    }

    fn class(&mut self) -> Option<()> {
        self.open("class");
        self.tok()?;
        self.tok()?;
        self.tok()?;
        while self.at_kw(&[Keyword::Static, Keyword::Field]) {
            self.open("classVarDec");
            self.through(b';')?;
            self.close("classVarDec");
        }
        while self.at_kw(&[Keyword::Constructor, Keyword::Function, Keyword::Method]) {
            self.subroutine()?;
        }
        self.tok()?;
        self.close("class");
        Some(())
    }

    fn subroutine(&mut self) -> Option<()> {
        self.open("subroutineDec");
        self.tok()?;
        self.tok()?;
        self.tok()?;
        self.tok()?; // (
        self.open("parameterList");
        while !self.at_sym(b')') {
            self.tok()?;
        }
        self.close("parameterList");
        self.tok()?; // )
        self.open("subroutineBody");
        self.tok()?; // {
        while self.at_kw(&[Keyword::Var]) {
            self.open("varDec");
            self.through(b';')?;
            self.close("varDec");
        }
        self.statements(0)?;
        self.tok()?; // }
        self.close("subroutineBody");
        self.close("subroutineDec");
        Some(())
    }

    fn statements(&mut self, depth: u32) -> Option<()> {
        if depth > crate::MAX_DEPTH {
            return None;
        }
        self.open("statements");
        while !self.at_sym(b'}') {
            let t = *self.peek()?;
            match t.kind {
                TokenKind::Keyword(Keyword::Let) => {
                    self.open("letStatement");
                    self.tok()?;
                    self.tok()?;
                    if self.at_sym(b'[') {
                        self.tok()?;
                        self.expression(depth + 1)?;
                        self.tok()?;
                    }
                    self.tok()?; // =
                    self.expression(depth + 1)?;
                    self.tok()?; // ;
                    self.close("letStatement");
                }
                TokenKind::Keyword(k @ (Keyword::If | Keyword::While)) => {
                    let tag = if k == Keyword::If {
                        "ifStatement"
                    } else {
                        "whileStatement"
                    };
                    self.open(tag);
                    self.tok()?;
                    self.tok()?; // (
                    self.expression(depth + 1)?;
                    self.tok()?; // )
                    self.tok()?; // {
                    self.statements(depth + 1)?;
                    self.tok()?; // }
                    if k == Keyword::If && self.at_kw(&[Keyword::Else]) {
                        self.tok()?;
                        self.tok()?;
                        self.statements(depth + 1)?;
                        self.tok()?;
                    }
                    self.close(tag);
                }
                TokenKind::Keyword(Keyword::Do) => {
                    self.open("doStatement");
                    self.tok()?;
                    self.call_rest(depth + 1)?;
                    self.tok()?; // ;
                    self.close("doStatement");
                }
                TokenKind::Keyword(Keyword::Return) => {
                    self.open("returnStatement");
                    self.tok()?;
                    if !self.at_sym(b';') {
                        self.expression(depth + 1)?;
                    }
                    self.tok()?;
                    self.close("returnStatement");
                }
                _ => return None,
            }
        }
        self.close("statements");
        Some(())
    }

    /// `name(args)` or `name.name(args)`, starting at the first name.
    fn call_rest(&mut self, depth: u32) -> Option<()> {
        self.tok()?;
        if self.at_sym(b'.') {
            self.tok()?;
            self.tok()?;
        }
        self.tok()?; // (
        self.open("expressionList");
        if !self.at_sym(b')') {
            self.expression(depth + 1)?;
            while self.at_sym(b',') {
                self.tok()?;
                self.expression(depth + 1)?;
            }
        }
        self.close("expressionList");
        self.tok() // )
    }

    fn expression(&mut self, depth: u32) -> Option<()> {
        if depth > crate::MAX_DEPTH * 2 {
            return None;
        }
        self.open("expression");
        self.term(depth + 1)?;
        while self
            .peek()
            .is_some_and(|t| matches!(t.kind, TokenKind::Symbol(c) if b"+-*/&|<>=".contains(&c)))
        {
            self.tok()?;
            self.term(depth + 1)?;
        }
        self.close("expression");
        Some(())
    }

    fn term(&mut self, depth: u32) -> Option<()> {
        if depth > crate::MAX_DEPTH * 2 {
            return None;
        }
        self.open("term");
        let t = *self.peek()?;
        match t.kind {
            TokenKind::Symbol(b'(') => {
                self.tok()?;
                self.expression(depth + 1)?;
                self.tok()?;
            }
            TokenKind::Symbol(b'-' | b'~') => {
                self.tok()?;
                self.term(depth + 1)?;
            }
            TokenKind::Identifier => {
                let next = self.toks.get(self.pos + 1);
                if next.is_some_and(|n| n.is_symbol(b'[')) {
                    self.tok()?;
                    self.tok()?;
                    self.expression(depth + 1)?;
                    self.tok()?;
                } else if next.is_some_and(|n| n.is_symbol(b'(') || n.is_symbol(b'.')) {
                    self.call_rest(depth + 1)?;
                } else {
                    self.tok()?;
                }
            }
            _ => self.tok()?,
        }
        self.close("term");
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_class() {
        let src = "class A { field int x; method int f(int a, char b) { var int y; \
                   let y = a < -x; do A.g(\"s&\", (y)); if (y) { return x[1]; } else { } \
                   while (true) { } return; } }";
        let x = parse_xml(src).unwrap();
        assert!(x.starts_with("<class>\n  <keyword> class </keyword>\n"));
        assert!(x.contains("<symbol> &lt; </symbol>"));
        assert!(x.contains("<stringConstant> s&amp; </stringConstant>"));
        assert!(x.contains("    <parameterList>\n      <keyword> int </keyword>\n"));
        assert!(x.ends_with("</class>\n"));
        let t = tokens_xml(src).unwrap();
        assert!(t.starts_with("<tokens>\n<keyword> class </keyword>\n"));
        assert!(parse_xml("class A {").is_err());
    }
}
