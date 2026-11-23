//! Line splitting and tokenisation, modelled on the observed behaviour of the
//! official assembler:
//!
//! - Lines end at `\n`, `\r\n` or a lone `\r`.
//! - Spaces and tabs are removed *before* anything else, wherever they are:
//!   `@5 6` is `@56`, `D = M` is `D=M`, and `@a/ /b` is `@a` plus a comment.
//!   No other character is whitespace (form feed, vertical tab, NBSP and NUL
//!   are ordinary characters and make the line invalid).
//! - `//` starts a comment that runs to the end of the line.
//! - A *word* is a maximal run of word characters: ASCII letters and digits,
//!   `_ . $ : + - ! & |`, and every character at or above U+0100. So `D+1`,
//!   `-1` and `!M` are single words, and so is a leading U+FEFF (BOM).
//! - Every other character (`@ ( ) ; =`, `/`, `#`, `é`, control characters…)
//!   is a token on its own.

use n2t_core::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokKind {
    Word,
    Char(char),
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: TokKind,
    /// The token text with any interior spaces and tabs removed.
    pub text: String,
    /// From the first to the last character of the token in the source.
    pub span: Span,
}

impl Token {
    pub fn is_char(&self, c: char) -> bool {
        self.kind == TokKind::Char(c)
    }
}

pub(crate) struct LexedLine {
    pub tokens: Vec<Token>,
    pub comment: Option<Span>,
}

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '.' | '$' | ':' | '+' | '-' | '!' | '&' | '|')
        || c as u32 >= 0x100
}

/// Byte ranges of the lines of `source`, without terminators.
pub(crate) fn split_lines(source: &str) -> Vec<Span> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(Span::new(start, i));
                i += 1;
                start = i;
            }
            b'\r' => {
                lines.push(Span::new(start, i));
                i += 1;
                if bytes.get(i) == Some(&b'\n') {
                    i += 1;
                }
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        lines.push(Span::new(start, bytes.len()));
    }
    lines
}

pub(crate) fn lex_line(source: &str, line: Span) -> LexedLine {
    let text = &source[line.start..line.end];
    // Significant characters with their byte offsets in `source`.
    let chars: Vec<(usize, char)> = text
        .char_indices()
        .filter(|&(_, c)| c != ' ' && c != '\t')
        .map(|(i, c)| (line.start + i, c))
        .collect();
    let mut end = chars.len();
    let mut comment = None;
    for i in 0..chars.len().saturating_sub(1) {
        if chars[i].1 == '/' && chars[i + 1].1 == '/' {
            end = i;
            comment = Some(Span::new(chars[i].0, line.end));
            break;
        }
    }
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < end {
        let (start, c) = chars[i];
        if is_word_char(c) {
            let mut text = String::new();
            let mut j = i;
            while j < end && is_word_char(chars[j].1) {
                text.push(chars[j].1);
                j += 1;
            }
            let (last, lc) = chars[j - 1];
            tokens.push(Token {
                kind: TokKind::Word,
                text,
                span: Span::new(start, last + lc.len_utf8()),
            });
            i = j;
        } else {
            tokens.push(Token {
                kind: TokKind::Char(c),
                text: c.to_string(),
                span: Span::new(start, start + c.len_utf8()),
            });
            i += 1;
        }
    }
    LexedLine { tokens, comment }
}

/// Java's `Integer.parseInt` followed by a 16-bit signed range check: an
/// optional `+` or `-`, then one or more decimal digits of any script in the
/// Basic Multilingual Plane (`@٣` is 3 in the official assembler).
/// Returns `None` if the text is not such a number or is out of range, in
/// which case the official assembler treats it as a symbol.
pub(crate) fn parse_number(text: &str) -> Option<i16> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i32 = 0;
    for c in digits.chars() {
        let d = decimal_digit(c)?;
        value = value * 10 + i32::from(d);
        if value > 32768 {
            return None;
        }
    }
    let value = if negative { -value } else { value };
    i16::try_from(value).ok()
}

/// The first code point of each run of ten decimal digits (Unicode category
/// Nd) in the BMP above U+00FF, as recognised by Java 17's `Character.digit`.
/// Verified against the official assembler, one probe per block.
const DIGIT_ZEROS: [u32; 36] = [
    0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66,
    0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
];

fn decimal_digit(c: char) -> Option<u8> {
    if c.is_ascii_digit() {
        return Some(c as u8 - b'0');
    }
    let cp = c as u32;
    DIGIT_ZEROS
        .iter()
        .find(|&&z| (z..z + 10).contains(&cp))
        .map(|&z| (cp - z) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        let l = lex_line(line, Span::new(0, line.len()));
        l.tokens.into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn splits_lines_like_read_line() {
        assert_eq!(split_lines("a\nb\r\nc\rd").len(), 4);
        assert_eq!(split_lines("a\n").len(), 1);
        assert_eq!(split_lines("").len(), 0);
        assert_eq!(split_lines("\n\n").len(), 2);
    }

    #[test]
    fn removes_whitespace_first() {
        assert_eq!(words("@5 6"), ["@", "56"]);
        assert_eq!(words(" D = M ; JMP "), ["D", "=", "M", ";", "JMP"]);
        assert_eq!(words("@a/ /b"), ["@", "a"]);
        assert_eq!(words("@1/2"), ["@", "1", "/", "2"]);
        assert_eq!(words("(a#b)"), ["(", "a", "#", "b", ")"]);
        assert_eq!(words("@aéb"), ["@", "a", "é", "b"]);
        assert_eq!(words("@aΔb"), ["@", "aΔb"]);
        assert_eq!(words("\u{feff}@1"), ["\u{feff}", "@", "1"]);
    }

    #[test]
    fn spans_cover_interior_whitespace() {
        let src = "@ 5 6 // x";
        let l = lex_line(src, Span::new(0, src.len()));
        assert_eq!(l.tokens[1].span, Span::new(2, 5));
        assert_eq!(l.comment, Some(Span::new(6, 10)));
    }

    #[test]
    fn parses_numbers_like_java() {
        assert_eq!(parse_number("32767"), Some(32767));
        assert_eq!(parse_number("32768"), None);
        assert_eq!(parse_number("-32768"), Some(-32768));
        assert_eq!(parse_number("-32769"), None);
        assert_eq!(parse_number("+5"), Some(5));
        assert_eq!(parse_number("-0"), Some(0));
        assert_eq!(parse_number("00000000000000000000001"), Some(1));
        assert_eq!(parse_number("٣٣"), Some(33));
        assert_eq!(parse_number("1１"), Some(11));
        assert_eq!(parse_number("𝟎"), None);
        assert_eq!(parse_number("+"), None);
        assert_eq!(parse_number("+-5"), None);
        assert_eq!(parse_number("1e3"), None);
        assert_eq!(parse_number("99999999999999999999"), None);
    }
}
