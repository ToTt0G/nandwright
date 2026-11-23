//! The Jack tokenizer.
//!
//! The official compiler's tokenizer behaves like Java's `StreamTokenizer`
//! configured for Jack, and this lexer reproduces that behaviour so that
//! every program is split into the same tokens:
//!
//! * whitespace is every character from U+0000 to U+0020;
//! * a word starts with an ASCII letter, `_`, a character from U+00A0 to
//!   U+00FF or any character above U+00FF, and continues with those and digits;
//!   U+0080..=U+009F, `$`, `@` and friends are single "unknown" characters;
//! * a number is a run of digits with at most one `.`; its value is computed as a
//!   double and truncated, so `1.5` is the integer `1`;
//! * `"…"` strings end at the closing quote, at the end of the line or at the end
//!   of the file (an unclosed string is not an error), and backslash escapes
//!   (`\n`, `\t`, `\\`, `\"`, octal `\101`, …) are interpreted;
//! * `'…'` is lexed the same way but is not a Jack token;
//! * `//` and `/* */` comments; block comments do not nest and an unclosed one
//!   silently runs to the end of the file.
//!
//! Each token also records the line number the official compiler would report
//! for it ([`Token::java_line`]), which differs from the real line in a few
//! corner cases (a newline escaped inside a string is not counted, and inside a
//! block comment a line break directly after another line break is not counted).

use n2t_core::Span;

/// The 21 reserved words of Jack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Class,
    Constructor,
    Function,
    Method,
    Field,
    Static,
    Var,
    Int,
    Char,
    Boolean,
    Void,
    True,
    False,
    Null,
    This,
    Let,
    Do,
    If,
    Else,
    While,
    Return,
}

impl Keyword {
    pub const ALL: [Keyword; 21] = [
        Keyword::Class,
        Keyword::Constructor,
        Keyword::Function,
        Keyword::Method,
        Keyword::Field,
        Keyword::Static,
        Keyword::Var,
        Keyword::Int,
        Keyword::Char,
        Keyword::Boolean,
        Keyword::Void,
        Keyword::True,
        Keyword::False,
        Keyword::Null,
        Keyword::This,
        Keyword::Let,
        Keyword::Do,
        Keyword::If,
        Keyword::Else,
        Keyword::While,
        Keyword::Return,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Keyword::Class => "class",
            Keyword::Constructor => "constructor",
            Keyword::Function => "function",
            Keyword::Method => "method",
            Keyword::Field => "field",
            Keyword::Static => "static",
            Keyword::Var => "var",
            Keyword::Int => "int",
            Keyword::Char => "char",
            Keyword::Boolean => "boolean",
            Keyword::Void => "void",
            Keyword::True => "true",
            Keyword::False => "false",
            Keyword::Null => "null",
            Keyword::This => "this",
            Keyword::Let => "let",
            Keyword::Do => "do",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::While => "while",
            Keyword::Return => "return",
        }
    }

    pub fn from_word(word: &str) -> Option<Keyword> {
        Keyword::ALL.into_iter().find(|k| k.as_str() == word)
    }
}

/// The Jack symbols.
pub const SYMBOLS: &str = "{}()[].,;+-*/&|<>=~";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Keyword(Keyword),
    /// One of [`SYMBOLS`]; the byte is the symbol character.
    Symbol(u8),
    Identifier,
    IntConst,
    StringConst,
    /// `// …` up to (not including) the line break.
    LineComment,
    /// `/* … */`.
    BlockComment,
    /// `/** … */`, the API documentation comment.
    DocComment,
    /// A character or `'…'` sequence that is not part of the Jack language.
    Unknown,
}

impl TokenKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            TokenKind::LineComment | TokenKind::BlockComment | TokenKind::DocComment
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// The 1-based line the official compiler associates with this token.
    pub java_line: u32,
}

impl Token {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.start..self.span.end]
    }

    pub fn is_symbol(&self, c: u8) -> bool {
        self.kind == TokenKind::Symbol(c)
    }

    pub fn is_keyword(&self, k: Keyword) -> bool {
        self.kind == TokenKind::Keyword(k)
    }
}

/// Lexical observations that are not errors for the official compiler but are
/// almost certainly mistakes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexNote {
    /// A string that ends at a line break or at the end of the file.
    UnclosedString(Span),
    /// A block comment that runs to the end of the file.
    UnclosedComment(Span),
    /// A backslash escape inside a string (span of the escape sequence).
    Escape(Span),
    /// A number with a fractional part, which the official compiler truncates.
    Fraction(Span),
}

#[derive(Debug, Clone)]
pub struct Lexed {
    /// All tokens, comments included, in source order.
    pub tokens: Vec<Token>,
    pub notes: Vec<LexNote>,
    /// The line the official compiler reports for errors at the end of the file.
    pub eof_java_line: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Whitespace,
    Alpha,
    Digit,
    Quote,
    Ordinary,
}

fn class_of(c: char) -> Class {
    let u = c as u32;
    match c {
        _ if u <= 32 => Class::Whitespace,
        'a'..='z' | 'A'..='Z' | '_' => Class::Alpha,
        '0'..='9' => Class::Digit,
        '"' | '\'' => Class::Quote,
        _ if u >= 0xA0 => Class::Alpha,
        _ => Class::Ordinary,
    }
}

struct Cursor<'a> {
    src: &'a str,
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    fn peek2(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next();
        it.next()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }
}

/// Splits `source` into tokens. Never fails: every byte ends up in a token,
/// in a comment or in whitespace.
pub fn lex(source: &str) -> Lexed {
    let mut cur = Cursor {
        src: source,
        pos: 0,
    };
    let mut tokens = Vec::new();
    let mut notes = Vec::new();
    let mut line: u32 = 1;

    while let Some(c) = cur.peek() {
        let start = cur.pos;
        match class_of(c) {
            Class::Whitespace => {
                cur.bump();
                if c == '\r' {
                    line = line.saturating_add(1);
                    if cur.peek() == Some('\n') {
                        cur.bump();
                    }
                } else if c == '\n' {
                    line = line.saturating_add(1);
                }
            }
            Class::Digit => {
                let mut seen_dot = false;
                let mut fraction = false;
                while let Some(d) = cur.peek() {
                    if d == '.' && !seen_dot {
                        seen_dot = true;
                    } else if d.is_ascii_digit() {
                        fraction |= seen_dot;
                    } else {
                        break;
                    }
                    cur.bump();
                }
                let span = Span::new(start, cur.pos);
                if seen_dot && (fraction || source[start..cur.pos].ends_with('.')) {
                    notes.push(LexNote::Fraction(span));
                }
                tokens.push(Token {
                    kind: TokenKind::IntConst,
                    span,
                    java_line: line,
                });
            }
            Class::Alpha => {
                while let Some(d) = cur.peek() {
                    if matches!(class_of(d), Class::Alpha | Class::Digit) {
                        cur.bump();
                    } else {
                        break;
                    }
                }
                let text = &source[start..cur.pos];
                let kind = match Keyword::from_word(text) {
                    Some(k) => TokenKind::Keyword(k),
                    None => TokenKind::Identifier,
                };
                tokens.push(Token {
                    kind,
                    span: Span::new(start, cur.pos),
                    java_line: line,
                });
            }
            Class::Quote => {
                cur.bump();
                let quote = c;
                let mut closed = false;
                while let Some(d) = cur.peek() {
                    if d == quote {
                        cur.bump();
                        closed = true;
                        break;
                    }
                    if d == '\n' || d == '\r' {
                        break;
                    }
                    if d == '\\' {
                        let esc_start = cur.pos;
                        cur.bump();
                        if let Some(e) = cur.bump()
                            && ('0'..='7').contains(&e)
                            && let Some(e2) = cur.peek()
                            && ('0'..='7').contains(&e2)
                        {
                            cur.bump();
                            if let Some(e3) = cur.peek()
                                && ('0'..='7').contains(&e3)
                                && e <= '3'
                            {
                                cur.bump();
                            }
                        }
                        if quote == '"' {
                            notes.push(LexNote::Escape(Span::new(esc_start, cur.pos)));
                        }
                    } else {
                        cur.bump();
                    }
                }
                let span = Span::new(start, cur.pos);
                if quote == '"' {
                    if !closed {
                        notes.push(LexNote::UnclosedString(span));
                    }
                    tokens.push(Token {
                        kind: TokenKind::StringConst,
                        span,
                        java_line: line,
                    });
                } else {
                    tokens.push(Token {
                        kind: TokenKind::Unknown,
                        span,
                        java_line: line,
                    });
                }
            }
            Class::Ordinary => {
                if c == '/' && cur.peek2() == Some('/') {
                    while let Some(d) = cur.peek() {
                        if d == '\n' || d == '\r' {
                            break;
                        }
                        cur.bump();
                    }
                    tokens.push(Token {
                        kind: TokenKind::LineComment,
                        span: Span::new(start, cur.pos),
                        java_line: line,
                    });
                } else if c == '/' && cur.peek2() == Some('*') {
                    let java_line = line;
                    cur.bump();
                    cur.bump();
                    // Reproduces StreamTokenizer's comment loop, including its
                    // line counting.
                    let mut prev: Option<char> = None;
                    let mut closed = false;
                    while let Some(mut d) = cur.bump() {
                        if d == '/' && prev == Some('*') {
                            closed = true;
                            break;
                        }
                        if d == '\r' {
                            line = line.saturating_add(1);
                            match cur.bump() {
                                Some('\n') => match cur.bump() {
                                    Some(x) => d = x,
                                    None => break,
                                },
                                Some(x) => d = x,
                                None => break,
                            }
                        } else if d == '\n' {
                            line = line.saturating_add(1);
                            match cur.bump() {
                                Some(x) => d = x,
                                None => break,
                            }
                        }
                        prev = Some(d);
                    }
                    let span = Span::new(start, cur.pos);
                    if !closed {
                        notes.push(LexNote::UnclosedComment(span));
                    }
                    let text = &source[start..cur.pos];
                    let doc = text.starts_with("/**") && text != "/**/";
                    tokens.push(Token {
                        kind: if doc {
                            TokenKind::DocComment
                        } else {
                            TokenKind::BlockComment
                        },
                        span,
                        java_line,
                    });
                } else {
                    cur.bump();
                    let kind = if c.is_ascii() && SYMBOLS.as_bytes().contains(&(c as u8)) {
                        TokenKind::Symbol(c as u8)
                    } else {
                        TokenKind::Unknown
                    };
                    tokens.push(Token {
                        kind,
                        span: Span::new(start, cur.pos),
                        java_line: line,
                    });
                }
            }
        }
    }

    Lexed {
        tokens,
        notes,
        eof_java_line: line,
    }
}

/// The value the official compiler gives an integer-constant token: the digits
/// (with an optional fraction) are evaluated as a double and truncated to a
/// 32-bit integer. Values above 32767 are rejected by the compiler.
pub fn int_value(text: &str) -> i32 {
    let mut v = 0f64;
    let mut decexp: i64 = 0;
    let mut seen_dot = false;
    for c in text.chars() {
        if c == '.' && !seen_dot {
            seen_dot = true;
        } else if let Some(d) = c.to_digit(10) {
            v = v * 10.0 + f64::from(d);
            if seen_dot {
                decexp += 1;
            }
        } else {
            break;
        }
    }
    if decexp != 0 {
        let mut denom = 10f64;
        decexp -= 1;
        while decexp > 0 {
            denom *= 10.0;
            decexp -= 1;
        }
        v /= denom;
    }
    // `as` saturates and maps NaN to 0, exactly like Java's (int) cast.
    v as i32
}

/// The UTF-16 code units of a string-constant token (`text` includes the
/// opening quote and, if present, the closing one), after escape processing.
pub fn string_units(text: &str) -> Vec<u16> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let Some(quote) = chars.next() else {
        return out;
    };
    let mut buf = [0u16; 2];
    while let Some(d) = chars.next() {
        if d == quote || d == '\n' || d == '\r' {
            break;
        }
        if d != '\\' {
            out.extend_from_slice(d.encode_utf16(&mut buf));
            continue;
        }
        let Some(c) = chars.next() else {
            // A backslash at the very end of the file reads end-of-file, which
            // StreamTokenizer stores as the character U+FFFF.
            out.push(0xFFFF);
            break;
        };
        if let Some(first) = c.to_digit(8) {
            let mut value = first;
            if let Some(d2) = chars.peek().and_then(|c2| c2.to_digit(8)) {
                chars.next();
                value = value * 8 + d2;
                if first <= 3
                    && let Some(d3) = chars.peek().and_then(|c3| c3.to_digit(8))
                {
                    chars.next();
                    value = value * 8 + d3;
                }
            }
            out.push(value as u16);
            continue;
        }
        let v = match c {
            'a' => 7,
            'b' => 8,
            'f' => 12,
            'n' => 10,
            'r' => 13,
            't' => 9,
            'v' => 11,
            other => {
                out.extend_from_slice(other.encode_utf16(&mut buf));
                continue;
            }
        };
        out.push(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokenKind, String)> {
        lex(src)
            .tokens
            .iter()
            .map(|t| (t.kind, t.text(src).to_string()))
            .collect()
    }

    #[test]
    fn words_numbers_symbols() {
        let k = kinds("let x_1 = a.b[3]-1.5;");
        let texts: Vec<_> = k.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            [
                "let", "x_1", "=", "a", ".", "b", "[", "3", "]", "-", "1.5", ";"
            ]
        );
        assert_eq!(k[0].0, TokenKind::Keyword(Keyword::Let));
        assert_eq!(k[10].0, TokenKind::IntConst);
        assert_eq!(int_value("1.5"), 1);
        assert_eq!(int_value("32767.9"), 32767);
        assert_eq!(int_value("99999999999999999999"), i32::MAX);
        assert_eq!(int_value("007"), 7);
    }

    #[test]
    fn number_followed_by_letters_splits() {
        let k = kinds("3x 0x10 1.2.3");
        let texts: Vec<_> = k.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, ["3", "x", "0", "x10", "1.2", ".", "3"]);
    }

    #[test]
    fn non_ascii_word_characters() {
        // U+00E9 is a word character, U+0085 and '$' are not.
        let k = kinds("caf\u{e9} a\u{85}b c$d");
        let texts: Vec<_> = k.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, ["caf\u{e9}", "a", "\u{85}", "b", "c", "$", "d"]);
        assert_eq!(k[2].0, TokenKind::Unknown);
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(string_units("\"a\\nb\""), vec![97, 10, 98]);
        assert_eq!(string_units("\"\\101\\0\\q\""), vec![65, 0, 113]);
        assert_eq!(string_units("\"\\\"x\""), vec![34, 120]);
        assert_eq!(string_units("\"\u{1F600}\""), vec![0xD83D, 0xDE00]);
        assert_eq!(string_units("\"\\477\""), vec![39, 55]);
        let l = lex("\"abc\nx");
        assert_eq!(l.tokens[0].text("\"abc\nx"), "\"abc");
        assert!(matches!(l.notes[0], LexNote::UnclosedString(_)));
        assert_eq!(l.tokens[1].java_line, 2);
    }

    #[test]
    fn comments_and_java_lines() {
        let src = "/** doc */ a // c\r\nb /* x\n\n\ny */ c\n";
        let l = lex(src);
        let toks: Vec<_> = l
            .tokens
            .iter()
            .map(|t| (t.kind, t.text(src), t.java_line))
            .collect();
        assert_eq!(toks[0].0, TokenKind::DocComment);
        assert_eq!(toks[1], (TokenKind::Identifier, "a", 1));
        assert_eq!(toks[2].0, TokenKind::LineComment);
        assert_eq!(toks[3], (TokenKind::Identifier, "b", 2));
        // Three line breaks inside the block comment, but the second one
        // directly follows the first and is not counted.
        assert_eq!(toks[5], (TokenKind::Identifier, "c", 4));
        assert_eq!(l.eof_java_line, 5);
    }

    #[test]
    fn unclosed_comment_runs_to_eof() {
        let l = lex("a /* b");
        assert_eq!(l.tokens.len(), 2);
        assert!(matches!(l.notes[0], LexNote::UnclosedComment(_)));
    }

    #[test]
    fn bom_is_part_of_a_word() {
        let src = "\u{feff}class";
        let l = lex(src);
        assert_eq!(l.tokens.len(), 1);
        assert_eq!(l.tokens[0].kind, TokenKind::Identifier);
    }
}
