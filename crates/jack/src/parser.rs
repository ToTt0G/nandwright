//! Recursive-descent parser for Jack with error recovery.
//!
//! The first syntax error is reported at the same token as the official
//! compiler. After an error the parser re-synchronises (at `;`, at a statement
//! keyword, at `}` or at a class-level keyword) so a half-typed file still
//! yields a tree for everything after the error. Missing punctuation is
//! reported and then assumed, so `if (x { … }` still produces an `if` with its
//! block.

use n2t_core::Span;

use crate::ast::*;
use crate::diag::{JDiag, codes, error, info, warning};
use crate::lexer::{self, Keyword, LexNote, Lexed, Token, TokenKind};

/// Maximum nesting of expressions, unary operators and blocks, counted
/// together. Deeper input gets a diagnostic instead of exhausting the stack
/// (the parser, checker and code generator are recursive). The official
/// compiler accepts deeper unary chains and blocks, but already breaks above 100
/// nested expressions; no real program comes close to either.
pub const MAX_DEPTH: u32 = 128;

pub(crate) struct ParseOutput {
    pub class: Option<Class>,
    pub diags: Vec<JDiag>,
}

pub(crate) fn parse_tokens(source: &str, lexed: &Lexed) -> ParseOutput {
    let mut toks = Vec::with_capacity(lexed.tokens.len());
    let mut docs = Vec::with_capacity(lexed.tokens.len());
    let mut pending_doc = None;
    for t in &lexed.tokens {
        match t.kind {
            TokenKind::DocComment => pending_doc = Some(t.span),
            TokenKind::LineComment | TokenKind::BlockComment => {}
            _ => {
                toks.push(*t);
                docs.push(pending_doc.take());
            }
        }
    }
    let mut p = Parser {
        src: source,
        toks,
        docs,
        pos: 0,
        diags: Vec::new(),
        last_error: None,
        depth: 0,
        depth_reported: false,
        eof_offset: source.len(),
        eof_line: lexed.eof_java_line,
    };
    for note in &lexed.notes {
        p.diags.push(lex_note(source, note));
    }
    let class = p.parse_file();
    ParseOutput {
        class,
        diags: p.diags,
    }
}

fn lex_note(source: &str, note: &LexNote) -> JDiag {
    match note {
        LexNote::UnclosedString(span) => warning(
            codes::UNCLOSED_STRING,
            "This string is not closed before the end of the line. The official compiler ends the \
             string here without an error; add the closing `\"`.",
            *span,
            0,
        ),
        LexNote::UnclosedComment(span) => warning(
            codes::UNCLOSED_COMMENT,
            "This comment is never closed, so everything up to the end of the file is part of it. \
             Add `*/` where the comment should end.",
            Span::new(span.start, (span.start + 2).min(span.end)),
            0,
        ),
        LexNote::Escape(span) => {
            let raw = span.slice(source);
            let units = lexer::string_units(&format!("\"{raw}\""));
            let text = raw.escape_debug().to_string();
            let shown = units
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            info(
                codes::STRING_ESCAPE,
                format!(
                    "Jack strings have no escape sequences, but the official compiler interprets \
                     `{text}` as character code {shown}. Use `String.appendChar` if you mean it."
                ),
                *span,
            )
        }
        LexNote::Fraction(span) => {
            let text = span.slice(source);
            warning(
                codes::FRACTION,
                format!(
                    "Jack has only integers. The official compiler truncates `{text}` to {}.",
                    lexer::int_value(text)
                ),
                *span,
                0,
            )
        }
    }
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    docs: Vec<Option<Span>>,
    pos: usize,
    diags: Vec<JDiag>,
    /// Token index of the last reported syntax error; further errors at the same
    /// token are suppressed to avoid cascades.
    last_error: Option<usize>,
    depth: u32,
    depth_reported: bool,
    eof_offset: usize,
    eof_line: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Statements,
}

impl<'a> Parser<'a> {
    // ---------------------------------------------------------------- helpers

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }
    fn peek_at(&self, n: usize) -> Option<&Token> {
        self.toks.get(self.pos + n)
    }
    fn at_symbol(&self, c: u8) -> bool {
        self.peek().is_some_and(|t| t.is_symbol(c))
    }
    fn at_keyword(&self, k: Keyword) -> bool {
        self.peek().is_some_and(|t| t.is_keyword(k))
    }
    fn at_eof(&self) -> bool {
        self.pos >= self.toks.len()
    }
    fn bump(&mut self) -> Option<Token> {
        let t = self.toks.get(self.pos).copied();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    /// Span of the current token, or an empty span at the end of the file.
    fn cur_span(&self) -> Span {
        match self.peek() {
            Some(t) => t.span,
            None => Span::empty(self.eof_offset),
        }
    }
    fn cur_line(&self) -> u32 {
        match self.peek() {
            Some(t) => t.java_line,
            None => self.eof_line,
        }
    }
    /// End offset of the previously consumed token.
    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].span.end
        }
    }
    fn text(&self, t: &Token) -> &'a str {
        t.text(self.src)
    }

    /// Describes the current token for an error message.
    fn describe_current(&self) -> String {
        match self.peek() {
            None => "the end of the file".to_string(),
            Some(t) => {
                let text = shorten(self.text(t));
                match t.kind {
                    TokenKind::Keyword(_) => format!("the keyword `{text}`"),
                    TokenKind::Symbol(_) => format!("`{text}`"),
                    TokenKind::Identifier => format!("`{text}`"),
                    TokenKind::IntConst => format!("the number `{text}`"),
                    TokenKind::StringConst => "a string".to_string(),
                    TokenKind::Unknown => format!("`{text}`"),
                    _ => format!("`{text}`"),
                }
            }
        }
    }

    /// Reports a syntax error at the current token (unless one was already
    /// reported there).
    fn error_here(&mut self, msg: impl Into<String>) {
        if self.last_error.is_some_and(|p| p >= self.pos) {
            return;
        }
        self.last_error = Some(self.pos);
        let span = self.cur_span();
        let line = self.cur_line();
        let mut msg = msg.into();
        if let Some(t) = self.peek() {
            let text = self.text(t);
            if text.starts_with('\u{feff}') {
                self.diags.push(error(
                    codes::BYTE_ORDER_MARK,
                    "The file starts with a UTF-8 byte order mark (BOM), which the official \
                     compiler cannot read. Save the file as UTF-8 without BOM.",
                    Span::new(span.start, span.start + 3),
                    line,
                ));
                return;
            }
            if t.kind == TokenKind::Unknown {
                msg = if text.starts_with('\'') {
                    format!(
                        "Jack has no character literals; use the character code instead \
                         (for example 65 for 'A'). {msg}"
                    )
                } else {
                    format!("`{}` is not a Jack symbol. {msg}", shorten(text))
                };
                self.diags
                    .push(error(codes::UNKNOWN_CHARACTER, msg, span, line));
                return;
            }
        }
        self.diags.push(error(codes::EXPECTED, msg, span, line));
    }

    /// [`Self::expected`] kept out of line so recursive callers stay small.
    #[cold]
    #[inline(never)]
    fn expected_cold(&mut self, what: &'static str) {
        self.expected(what);
    }

    #[cold]
    #[inline(never)]
    fn term_expected(&mut self, keyword: bool) {
        let found = self.describe_current();
        let hint = if keyword {
            " Keywords other than `true`, `false`, `null` and `this` cannot be values."
        } else {
            ""
        };
        self.error_here(format!(
            "Expected an expression (a constant, variable, call, `(`, `-` or `~`), found \
             {found}.{hint}"
        ));
    }

    fn expected(&mut self, what: &str) {
        let found = self.describe_current();
        self.error_here(format!("Expected {what}, found {found}."));
    }

    fn expect_symbol(&mut self, c: u8, what: &str) -> Option<Span> {
        if self.at_symbol(c) {
            return self.bump().map(|t| t.span);
        }
        self.expected(what);
        None
    }

    fn expect_ident(&mut self, what: &str) -> Option<Ident> {
        match self.peek() {
            Some(t) if t.kind == TokenKind::Identifier => {
                let t = *t;
                self.pos += 1;
                Some(Ident {
                    name: self.text(&t).to_string(),
                    span: t.span,
                })
            }
            _ => {
                self.expected(what);
                None
            }
        }
    }

    fn enter(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            if !self.depth_reported {
                self.depth_reported = true;
                let span = self.cur_span();
                let line = self.cur_line();
                self.diags.push(error(
                    codes::TOO_DEEP,
                    format!(
                        "This is nested more than {MAX_DEPTH} levels deep, which this compiler \
                         does not support (the official one does). Split the expression or \
                         statement into smaller parts."
                    ),
                    span,
                    line,
                ));
                self.last_error = Some(self.pos);
            }
            return false;
        }
        self.depth += 1;
        true
    }
    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn is_statement_start(t: &Token) -> bool {
        matches!(
            t.kind,
            TokenKind::Keyword(
                Keyword::Let | Keyword::Do | Keyword::If | Keyword::While | Keyword::Return
            )
        )
    }
    fn is_class_level(t: &Token) -> bool {
        matches!(
            t.kind,
            TokenKind::Keyword(
                Keyword::Constructor
                    | Keyword::Function
                    | Keyword::Method
                    | Keyword::Field
                    | Keyword::Static
                    | Keyword::Class
            )
        )
    }

    /// Skips tokens after an error until a sensible restart point. A `;` is
    /// consumed; statement keywords, braces and class-level keywords are not.
    fn synchronize(&mut self, ctx: Ctx) {
        while let Some(t) = self.peek() {
            if t.is_symbol(b';') {
                self.pos += 1;
                return;
            }
            if t.is_symbol(b'}') || t.is_symbol(b'{') || Self::is_class_level(t) {
                return;
            }
            if ctx == Ctx::Statements && (Self::is_statement_start(t) || t.is_keyword(Keyword::Var))
            {
                return;
            }
            self.pos += 1;
        }
    }

    // ------------------------------------------------------------------ class

    fn parse_file(&mut self) -> Option<Class> {
        if !self.at_keyword(Keyword::Class) {
            self.expected("`class`");
            while let Some(t) = self.peek() {
                if t.is_keyword(Keyword::Class) {
                    break;
                }
                self.pos += 1;
            }
            if self.at_eof() {
                return None;
            }
        }
        let class = self.parse_class();
        if !self.at_eof() {
            // The official compiler reports this on the line of the token
            // before (the class's closing brace).
            let prev_line = self
                .pos
                .checked_sub(1)
                .and_then(|i| self.toks.get(i))
                .map(|t| t.java_line);
            let n = self.diags.len();
            self.error_here(format!(
                "Expected the end of the file after the class's closing `}}`, found {}. A file \
                 holds exactly one class.",
                self.describe_current()
            ));
            if let (Some(l), Some(d)) = (prev_line, self.diags.get_mut(n)) {
                d.java_line = l;
            }
        }
        Some(class)
    }

    fn parse_class(&mut self) -> Class {
        let doc = self.docs.get(self.pos).copied().flatten();
        let start = self.cur_span().start;
        self.bump(); // `class`
        let name = self.expect_ident("a class name after `class`");
        let open = self.expect_symbol(b'{', "`{` to start the class body");
        let mut vars = Vec::new();
        let mut subroutines = Vec::new();
        let mut seen_subroutine = false;
        while let Some(t) = self.peek().copied() {
            match t.kind {
                TokenKind::Keyword(Keyword::Static | Keyword::Field) => {
                    if seen_subroutine {
                        self.error_here(
                            "Expected `}`: field and static declarations must come before the \
                             subroutines.",
                        );
                    }
                    vars.push(self.parse_class_var());
                }
                TokenKind::Keyword(Keyword::Constructor | Keyword::Function | Keyword::Method) => {
                    seen_subroutine = true;
                    subroutines.push(self.parse_subroutine());
                }
                TokenKind::Symbol(b'}') => break,
                TokenKind::Keyword(Keyword::Class) => break,
                _ => {
                    if seen_subroutine {
                        self.expected("`}` to close the class, or another subroutine");
                    } else {
                        self.expected(
                            "`field`, `static`, `constructor`, `function`, `method` or `}`",
                        );
                    }
                    // Skip to the next class-level keyword or brace.
                    self.pos += 1;
                    while let Some(t) = self.peek() {
                        if Self::is_class_level(t) || t.is_symbol(b'}') {
                            break;
                        }
                        self.pos += 1;
                    }
                }
            }
        }
        let close = if self.at_symbol(b'}') {
            self.bump().map(|t| t.span)
        } else {
            let mut msg = "Expected `}` to close the class".to_string();
            if open.is_some() {
                msg.push_str(&format!(", found {}.", self.describe_current()));
            } else {
                msg.push('.');
            }
            self.error_here(msg);
            None
        };
        let end = close.map_or(self.prev_end(), |s| s.end);
        Class {
            name,
            vars,
            subroutines,
            span: Span::new(start, end.max(start)),
            doc,
        }
    }

    fn parse_type(&mut self, what: &str) -> Option<Type> {
        let t = *self.peek()?;
        let kind = match t.kind {
            TokenKind::Keyword(Keyword::Int) => TypeKind::Int,
            TokenKind::Keyword(Keyword::Char) => TypeKind::Char,
            TokenKind::Keyword(Keyword::Boolean) => TypeKind::Boolean,
            TokenKind::Identifier => TypeKind::Class(self.text(&t).to_string()),
            _ => {
                if t.is_keyword(Keyword::Void) {
                    self.error_here(format!(
                        "`void` can only be the return type of a subroutine; {what} needs `int`, \
                         `char`, `boolean` or a class name."
                    ));
                } else {
                    self.expected(&format!(
                        "{what} (`int`, `char`, `boolean` or a class name)"
                    ));
                }
                return None;
            }
        };
        self.pos += 1;
        Some(Type { kind, span: t.span })
    }

    fn parse_class_var(&mut self) -> ClassVarDec {
        let doc = self.docs.get(self.pos).copied().flatten();
        let kw = self.bump().expect("caller checked");
        let kind = if kw.is_keyword(Keyword::Static) {
            ClassVarKind::Static
        } else {
            ClassVarKind::Field
        };
        let (ty, names) = self.parse_var_names(kw.span.start, "the variable's type");
        ClassVarDec {
            kind,
            ty,
            names,
            span: Span::new(kw.span.start, self.prev_end().max(kw.span.start)),
            doc,
        }
    }

    /// Parses `type name (, name)* ;` (after `var`, `field` or `static`).
    fn parse_var_names(&mut self, _start: usize, what: &str) -> (Option<Type>, Vec<Ident>) {
        let mut names = Vec::new();
        let Some(ty) = self.parse_type(what) else {
            self.synchronize(Ctx::Statements);
            return (None, names);
        };
        loop {
            match self.expect_ident("a variable name") {
                Some(id) => names.push(id),
                None => {
                    self.synchronize(Ctx::Statements);
                    return (Some(ty), names);
                }
            }
            if self.at_symbol(b',') {
                self.pos += 1;
                continue;
            }
            if self.at_symbol(b';') {
                self.pos += 1;
            } else {
                self.expected("`,` or `;` after the variable name");
                self.synchronize(Ctx::Statements);
            }
            return (Some(ty), names);
        }
    }

    // ------------------------------------------------------------ subroutine

    fn parse_subroutine(&mut self) -> Subroutine {
        let doc = self.docs.get(self.pos).copied().flatten();
        let kw = self.bump().expect("caller checked");
        let kind = match kw.kind {
            TokenKind::Keyword(Keyword::Constructor) => SubroutineKind::Constructor,
            TokenKind::Keyword(Keyword::Function) => SubroutineKind::Function,
            _ => SubroutineKind::Method,
        };
        let mut sub = Subroutine {
            kind,
            kind_span: kw.span,
            return_type: None,
            name: None,
            params: Vec::new(),
            locals: Vec::new(),
            statements: Vec::new(),
            body_open: None,
            body_close: None,
            span: kw.span,
            doc,
        };
        let finish = |p: &mut Self, mut sub: Subroutine| {
            sub.span = Span::new(kw.span.start, p.prev_end().max(kw.span.end));
            sub
        };

        if self.at_keyword(Keyword::Void) {
            let t = self.bump().expect("checked");
            sub.return_type = Some(ReturnType::Void(t.span));
        } else {
            match self.parse_type("the return type (`void` or a type)") {
                Some(ty) => sub.return_type = Some(ReturnType::Type(ty)),
                None => {
                    // `function main() {` — keep going if a name follows.
                    if !(self.peek().is_some_and(|t| t.kind == TokenKind::Identifier)
                        && self.peek_at(1).is_some_and(|t| t.is_symbol(b'(')))
                    {
                        self.skip_to_body_or_next(&mut sub);
                        return finish(self, sub);
                    }
                }
            }
        }
        sub.name = self.expect_ident("the subroutine name");
        if sub.name.is_none() && !self.at_symbol(b'(') {
            self.skip_to_body_or_next(&mut sub);
            return finish(self, sub);
        }
        if self
            .expect_symbol(b'(', "`(` to start the parameter list")
            .is_some()
        {
            self.parse_params(&mut sub);
        } else if !self.at_symbol(b'{') {
            self.skip_to_body_or_next(&mut sub);
            return finish(self, sub);
        }
        self.parse_body(&mut sub);
        finish(self, sub)
    }

    /// Recovery inside a subroutine header: skip to its body (and parse it) or
    /// to the next class-level declaration.
    fn skip_to_body_or_next(&mut self, sub: &mut Subroutine) {
        while let Some(t) = self.peek() {
            if t.is_symbol(b'{') {
                self.parse_body(sub);
                return;
            }
            if Self::is_class_level(t) || t.is_symbol(b'}') {
                return;
            }
            self.pos += 1;
        }
    }

    fn parse_params(&mut self, sub: &mut Subroutine) {
        if self.at_symbol(b')') {
            self.pos += 1;
            return;
        }
        loop {
            let start = self.cur_span().start;
            let ty = self.parse_type("a parameter type");
            let name = if ty.is_some() {
                self.expect_ident("a parameter name")
            } else {
                None
            };
            let ok = ty.is_some() && name.is_some();
            sub.params.push(Param {
                ty,
                name,
                span: Span::new(start, self.prev_end().max(start)),
            });
            if !ok {
                // Skip to `)` or `{`.
                while let Some(t) = self.peek() {
                    if t.is_symbol(b')') {
                        self.pos += 1;
                        return;
                    }
                    if t.is_symbol(b'{') || Self::is_class_level(t) || t.is_symbol(b'}') {
                        return;
                    }
                    self.pos += 1;
                }
                return;
            }
            if self.at_symbol(b',') {
                self.pos += 1;
                continue;
            }
            if self.at_symbol(b')') {
                self.pos += 1;
                return;
            }
            self.expected("`)` or `,` in the parameter list");
            while let Some(t) = self.peek() {
                if t.is_symbol(b')') {
                    self.pos += 1;
                    return;
                }
                if t.is_symbol(b'{') || Self::is_class_level(t) || t.is_symbol(b'}') {
                    return;
                }
                self.pos += 1;
            }
            return;
        }
    }

    fn parse_body(&mut self, sub: &mut Subroutine) {
        sub.body_open = self.expect_symbol(b'{', "`{` to start the subroutine body");
        if sub.body_open.is_none()
            && !self
                .peek()
                .is_some_and(|t| t.is_keyword(Keyword::Var) || Self::is_statement_start(t))
        {
            return;
        }
        while self.at_keyword(Keyword::Var) {
            let start = self.cur_span().start;
            self.pos += 1;
            let (ty, names) = self.parse_var_names(start, "the variable's type");
            sub.locals.push(VarDec {
                ty,
                names,
                span: Span::new(start, self.prev_end().max(start)),
            });
        }
        sub.statements = self.parse_statements(Some(&mut sub.locals));
        if self.at_symbol(b'}') {
            sub.body_close = self.bump().map(|t| t.span);
        } else {
            self.expected("`}` to close the subroutine body");
        }
    }

    // ------------------------------------------------------------- statements

    /// Parses statements until `}`, a class-level keyword or the end of file.
    /// `var` declarations found among statements are reported and, when
    /// `locals` is given, still recorded so their names resolve.
    fn parse_statements(&mut self, mut locals: Option<&mut Vec<VarDec>>) -> Vec<Statement> {
        let mut out = Vec::new();
        while let Some(t) = self.peek().copied() {
            if t.is_symbol(b'}') || Self::is_class_level(&t) {
                break;
            }
            if t.is_keyword(Keyword::Var) {
                self.error_here(
                    "Expected a statement: `var` declarations must come before the first statement \
                     of the subroutine.",
                );
                let start = t.span.start;
                self.pos += 1;
                let (ty, names) = self.parse_var_names(start, "the variable's type");
                if let Some(l) = locals.as_deref_mut() {
                    l.push(VarDec {
                        ty,
                        names,
                        span: Span::new(start, self.prev_end().max(start)),
                    });
                }
                continue;
            }
            let before = self.pos;
            match self.parse_statement() {
                Some(s) => out.push(s),
                None => {
                    if self.pos == before {
                        if t.is_symbol(b'{') {
                            // A stray block: parse it so its statements are checked.
                            let b = self.parse_block("nested");
                            out.extend(b.statements);
                            continue;
                        }
                        // No progress: skip the offending token.
                        self.pos += 1;
                    }
                    self.synchronize(Ctx::Statements);
                }
            }
        }
        out
    }

    fn parse_statement(&mut self) -> Option<Statement> {
        let t = *self.peek()?;
        match t.kind {
            TokenKind::Keyword(Keyword::Let) => Some(self.parse_let()),
            TokenKind::Keyword(Keyword::If) => Some(self.parse_if()),
            TokenKind::Keyword(Keyword::While) => Some(self.parse_while()),
            TokenKind::Keyword(Keyword::Do) => Some(self.parse_do()),
            TokenKind::Keyword(Keyword::Return) => Some(self.parse_return()),
            TokenKind::Keyword(Keyword::Else) => {
                self.error_here("Expected a statement: this `else` has no matching `if`.");
                None
            }
            TokenKind::Identifier
                if self
                    .peek_at(1)
                    .is_some_and(|n| n.is_symbol(b'=') || n.is_symbol(b'[')) =>
            {
                self.error_here(format!(
                    "Expected a statement (`let`, `do`, `if`, `while` or `return`). To assign \
                     to `{}`, write `let {} = …;`.",
                    self.text(&t),
                    self.text(&t)
                ));
                None
            }
            TokenKind::Identifier
                if self
                    .peek_at(1)
                    .is_some_and(|n| n.is_symbol(b'(') || n.is_symbol(b'.')) =>
            {
                self.error_here(
                    "Expected a statement (`let`, `do`, `if`, `while` or `return`). To call a \
                     subroutine as a statement, write `do …;`.",
                );
                None
            }
            _ => {
                let found = self.describe_current();
                self.error_here(format!(
                    "Expected a statement (`let`, `do`, `if`, `while` or `return`), found {found}."
                ));
                None
            }
        }
    }

    fn end_statement(&mut self, what: &str) -> Option<Span> {
        if self.at_symbol(b';') {
            return self.bump().map(|t| t.span);
        }
        let found = self.describe_current();
        self.error_here(format!(
            "Expected `;` at the end of the `{what}` statement, found {found}."
        ));
        self.synchronize(Ctx::Statements);
        None
    }

    fn parse_let(&mut self) -> Statement {
        let kw = self.bump().expect("checked");
        let name = match self.expect_ident("a variable name after `let`") {
            Some(id) => id,
            None => {
                let empty = Ident {
                    name: String::new(),
                    span: Span::empty(kw.span.end),
                };
                self.synchronize(Ctx::Statements);
                return Statement::Let {
                    name: empty,
                    index: None,
                    value: None,
                    semicolon: None,
                    span: Span::new(kw.span.start, self.prev_end()),
                };
            }
        };
        let mut index = None;
        if self.at_symbol(b'[') {
            let open = self.bump().expect("checked").span;
            index = Some(self.parse_expr());
            if !self.at_symbol(b']') {
                let found = self.describe_current();
                self.error_here(format!(
                    "Expected `]` to close the array index, found {found}."
                ));
                if let Some(d) = self.diags.last_mut()
                    && d.diag.related.is_empty()
                {
                    d.diag = d.diag.clone().with_related_span("`[` opened here", open);
                }
            } else {
                self.pos += 1;
            }
        }
        let mut value = None;
        let mut semicolon = None;
        if self.at_symbol(b'=') {
            self.pos += 1;
            value = Some(self.parse_expr());
            semicolon = self.end_statement("let");
        } else {
            let found = self.describe_current();
            self.error_here(if index.is_some() {
                format!("Expected `=` after the array element, found {found}.")
            } else {
                format!("Expected `[` or `=` after the variable name, found {found}.")
            });
            self.synchronize(Ctx::Statements);
        }
        Statement::Let {
            name,
            index,
            value,
            semicolon,
            span: Span::new(kw.span.start, self.prev_end()),
        }
    }

    /// `( expr )` of `if` and `while`.
    fn parse_condition(&mut self, what: &str) -> Option<Expr> {
        let open = self.expect_symbol(b'(', &format!("`(` after `{what}`"));
        if open.is_none() && self.at_symbol(b'{') {
            return None;
        }
        let e = self.parse_expr();
        if self.at_symbol(b')') {
            self.pos += 1;
        } else {
            let found = self.describe_current();
            self.error_here(format!(
                "Expected `)` to close the `{what}` condition, found {found}."
            ));
        }
        Some(e)
    }

    fn parse_block(&mut self, what: &str) -> Block {
        let start = self.cur_span().start;
        if self.at_symbol(b'{') {
            self.pos += 1;
        } else {
            let found = self.describe_current();
            self.error_here(format!(
                "Expected `{{` to start the {what} block, found {found}. Jack needs braces even \
                 around a single statement."
            ));
            // Braceless body: take a single statement.
            if self.peek().is_some_and(Self::is_statement_start) {
                if !self.enter() {
                    return Block {
                        statements: Vec::new(),
                        span: Span::empty(start),
                    };
                }
                let s = self.parse_statement();
                self.leave();
                return Block {
                    statements: s.into_iter().collect(),
                    span: Span::new(start, self.prev_end().max(start)),
                };
            }
            return Block {
                statements: Vec::new(),
                span: Span::empty(start),
            };
        }
        if !self.enter() {
            self.skip_balanced_block();
            return Block {
                statements: Vec::new(),
                span: Span::new(start, self.prev_end()),
            };
        }
        let statements = self.parse_statements(None);
        self.leave();
        if self.at_symbol(b'}') {
            self.pos += 1;
        } else {
            let found = self.describe_current();
            self.error_here(format!(
                "Expected `}}` to close the {what} block, found {found}."
            ));
        }
        Block {
            statements,
            span: Span::new(start, self.prev_end().max(start)),
        }
    }

    /// Skips to the brace that closes the block whose `{` was just consumed,
    /// without recursion.
    fn skip_balanced_block(&mut self) {
        let mut level = 1usize;
        while let Some(t) = self.bump() {
            if t.is_symbol(b'{') {
                level += 1;
            } else if t.is_symbol(b'}') {
                level -= 1;
                if level == 0 {
                    return;
                }
            }
        }
    }

    fn parse_if(&mut self) -> Statement {
        let kw = self.bump().expect("checked");
        let cond = self.parse_condition("if");
        let then_block = self.parse_block("`if`");
        let mut else_block = None;
        if self.at_keyword(Keyword::Else) {
            self.pos += 1;
            else_block = Some(self.parse_block("`else`"));
        }
        Statement::If {
            cond,
            then_block,
            else_block,
            span: Span::new(kw.span.start, self.prev_end()),
        }
    }

    fn parse_while(&mut self) -> Statement {
        let kw = self.bump().expect("checked");
        let cond = self.parse_condition("while");
        let body = self.parse_block("`while`");
        Statement::While {
            cond,
            body,
            span: Span::new(kw.span.start, self.prev_end()),
        }
    }

    fn parse_do(&mut self) -> Statement {
        let kw = self.bump().expect("checked");
        let call = match self.peek().copied() {
            Some(t) if t.kind == TokenKind::Identifier => {
                self.pos += 1;
                let first = Ident {
                    name: self.text(&t).to_string(),
                    span: t.span,
                };
                if self.at_symbol(b'(') || self.at_symbol(b'.') {
                    self.parse_call(first)
                } else {
                    let found = self.describe_current();
                    self.error_here(format!(
                        "Expected `(` or `.`: `do` must be followed by a subroutine call, found {found}."
                    ));
                    None
                }
            }
            _ => {
                let found = self.describe_current();
                self.error_here(format!(
                    "Expected a subroutine call after `do` (a subroutine, class or variable name), \
                     found {found}."
                ));
                None
            }
        };
        if call.is_none() {
            self.synchronize(Ctx::Statements);
            return Statement::Do {
                call,
                span: Span::new(kw.span.start, self.prev_end()),
            };
        }
        self.end_statement("do");
        Statement::Do {
            call,
            span: Span::new(kw.span.start, self.prev_end()),
        }
    }

    fn parse_return(&mut self) -> Statement {
        let kw = self.bump().expect("checked");
        let mut value = None;
        if !self.at_symbol(b';') {
            value = Some(self.parse_expr());
        }
        let semicolon = self.end_statement("return");
        Statement::Return {
            value,
            semicolon,
            span: Span::new(kw.span.start, self.prev_end()),
        }
    }

    // ------------------------------------------------------------ expressions

    fn parse_expr(&mut self) -> Expr {
        let start = self.cur_span().start;
        if !self.enter() {
            return Expr {
                first: Term::Missing(Span::empty(start)),
                rest: Vec::new(),
                span: Span::empty(start),
            };
        }
        let first = self.parse_term();
        let mut rest = Vec::new();
        let mut ok = !matches!(first, Term::Missing(_));
        while ok {
            let Some(op) = self.peek().and_then(|t| match t.kind {
                TokenKind::Symbol(c) => BinOp::from_symbol(c),
                _ => None,
            }) else {
                break;
            };
            let op_span = self.bump().expect("checked").span;
            let term = self.parse_term();
            ok = !matches!(term, Term::Missing(_));
            rest.push((op, op_span, term));
        }
        self.leave();
        let end = self.prev_end().max(start);
        Expr {
            first,
            rest,
            span: Span::new(start, end),
        }
    }

    fn parse_term(&mut self) -> Term {
        let Some(t) = self.peek().copied() else {
            self.error_here("Expected an expression, found the end of the file.");
            return Term::Missing(Span::empty(self.eof_offset));
        };
        match t.kind {
            TokenKind::IntConst => {
                self.pos += 1;
                Term::Int {
                    value: lexer::int_value(self.text(&t)),
                    span: t.span,
                }
            }
            TokenKind::StringConst => {
                self.pos += 1;
                Term::Str {
                    units: lexer::string_units(self.text(&t)),
                    span: t.span,
                }
            }
            TokenKind::Keyword(
                k @ (Keyword::True | Keyword::False | Keyword::Null | Keyword::This),
            ) => {
                self.pos += 1;
                let value = match k {
                    Keyword::True => KeywordConst::True,
                    Keyword::False => KeywordConst::False,
                    Keyword::Null => KeywordConst::Null,
                    _ => KeywordConst::This,
                };
                Term::Keyword {
                    value,
                    span: t.span,
                }
            }
            TokenKind::Identifier => {
                self.pos += 1;
                let id = Ident {
                    name: self.text(&t).to_string(),
                    span: t.span,
                };
                if self.at_symbol(b'[') {
                    self.pos += 1;
                    let index = self.parse_expr();
                    if self.at_symbol(b']') {
                        self.pos += 1;
                    } else {
                        self.expected_cold("`]` to close the array index");
                    }
                    let span = Span::new(t.span.start, self.prev_end());
                    Term::Index {
                        name: id,
                        index: Box::new(index),
                        span,
                    }
                } else if self.at_symbol(b'(') || self.at_symbol(b'.') {
                    match self.parse_call(id.clone()) {
                        Some(c) => Term::Call(Box::new(c)),
                        None => Term::Var(id),
                    }
                } else {
                    Term::Var(id)
                }
            }
            TokenKind::Symbol(b'(') => {
                self.pos += 1;
                let e = self.parse_expr();
                if self.at_symbol(b')') {
                    self.pos += 1;
                } else {
                    self.expected_cold("`)` to close the parenthesis");
                }
                Term::Paren {
                    expr: Box::new(e),
                    span: Span::new(t.span.start, self.prev_end()),
                }
            }
            TokenKind::Symbol(c @ (b'-' | b'~')) => {
                self.pos += 1;
                if !self.enter() {
                    return Term::Missing(t.span);
                }
                let inner = self.parse_term();
                self.leave();
                if let Term::Missing(_) = inner {
                    return inner;
                }
                let span = Span::new(t.span.start, inner.span().end);
                Term::Unary {
                    op: if c == b'-' {
                        UnaryOp::Neg
                    } else {
                        UnaryOp::Not
                    },
                    op_span: t.span,
                    term: Box::new(inner),
                    span,
                }
            }
            _ => {
                self.term_expected(matches!(t.kind, TokenKind::Keyword(_)));
                Term::Missing(Span::empty(t.span.start))
            }
        }
    }

    /// Parses the rest of a call after its first identifier (the current token
    /// is `(` or `.`).
    fn parse_call(&mut self, first: Ident) -> Option<Call> {
        let start = first.span.start;
        let (receiver, name) = if self.at_symbol(b'.') {
            let dot = self.bump().expect("checked");
            match self.peek().copied() {
                Some(t) if t.kind == TokenKind::Identifier => {
                    self.pos += 1;
                    (
                        Some(first),
                        Ident {
                            name: self.text(&t).to_string(),
                            span: t.span,
                        },
                    )
                }
                _ => {
                    self.expected_cold("a subroutine name after `.`");
                    // Keep a call with an empty name so editors can complete it.
                    return Some(Call {
                        receiver: Some(first),
                        name: Ident {
                            name: String::new(),
                            span: Span::empty(dot.span.end),
                        },
                        args: Vec::new(),
                        open: None,
                        close: None,
                        span: Span::new(start, dot.span.end),
                    });
                }
            }
        } else {
            (None, first)
        };
        let mut call = Call {
            receiver,
            name,
            args: Vec::new(),
            open: None,
            close: None,
            span: Span::new(start, self.prev_end()),
        };
        if !self.at_symbol(b'(') {
            self.expected_cold("`(` after the subroutine name");
            return Some(call);
        }
        call.open = self.bump().map(|t| t.span);
        if !self.at_symbol(b')') {
            loop {
                let e = self.parse_expr();
                let missing = matches!(e.first, Term::Missing(_)) && e.rest.is_empty();
                if !missing {
                    call.args.push(e);
                }
                if self.at_symbol(b',') {
                    self.pos += 1;
                    continue;
                }
                break;
            }
        }
        if self.at_symbol(b')') {
            call.close = self.bump().map(|t| t.span);
        } else {
            self.expected_cold("`)` or `,` in the argument list");
        }
        call.span = Span::new(start, self.prev_end());
        Some(call)
    }
}

/// Token text quoted in a message: whitespace collapsed and at most 24 characters.
fn shorten(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 24 {
        let mut s: String = collapsed.chars().take(23).collect();
        s.push('…');
        s
    } else {
        collapsed
    }
}
