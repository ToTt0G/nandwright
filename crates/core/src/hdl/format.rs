//! Formatter for `.hdl` chip files.
//!
//! ```hdl
//! CHIP Nor {
//!     IN a, b;
//!     OUT out;
//!
//!     PARTS:
//!     Or(a=a, b=b, out=either);
//!     Not(in=either, out=out);
//! }
//! ```
//!
//! It fixes indentation and the spacing inside each line, removes trailing
//! whitespace and collapses runs of blank lines. It never moves a token or a
//! comment to another line (see [`crate::format`]), and it refuses to touch a
//! file with syntax errors or one whose tokens would not survive unchanged.

use crate::diagnostic::Severity;
use crate::format::{Class, FormatOptions, Gap, Piece, comment_key, reformat, same_tokens};
use crate::hdl::lexer::{Token, TokenContent};

/// Formats an `.hdl` source using the default [`FormatOptions`].
///
/// Returns `None` if the source contains lexical or syntax errors.
pub fn format(source: &str) -> Option<String> {
    format_with_options(source, &FormatOptions::default())
}

/// Formats an `.hdl` source with custom [`FormatOptions`].
///
/// Returns `None` if the source has lexical or syntax errors, or if the
/// formatted text would not lex to exactly the same tokens and comments.
pub fn format_with_options(source: &str, options: &FormatOptions) -> Option<String> {
    let parsed = crate::hdl::parse(source);
    if parsed.chip.is_none()
        || parsed
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    {
        return None;
    }
    let pieces: Vec<Piece> = parsed.tokens.iter().filter_map(piece).collect();
    let formatted = reformat(source, &pieces, options, &gap);
    let after = crate::hdl::lexer::lex(&formatted);
    if !after.diagnostics.is_empty()
        || !same_tokens(
            &keys(source, &parsed.tokens),
            &keys(&formatted, &after.tokens),
        )
    {
        return None;
    }
    Some(formatted)
}

fn piece(token: &Token) -> Option<Piece> {
    use TokenContent::*;
    let (class, ch, ends_statement) = match &token.content {
        Eof => return None,
        Comment(_) | DocComment(_) => (Class::Comment, 0, false),
        Keyword(_) | Ident(_) | Integer(_) => (Class::Word, 0, false),
        LeftBrace => (Class::Open, b'{', true),
        RightBrace => (Class::Close, b'}', true),
        LeftParen => (Class::Punct, b'(', false),
        RightParen => (Class::Punct, b')', false),
        LeftBracket => (Class::Punct, b'[', false),
        RightBracket => (Class::Punct, b']', false),
        Comma => (Class::Punct, b',', false),
        Semicolon => (Class::Punct, b';', true),
        // `PARTS:` is a line of its own.
        Colon => (Class::Punct, b':', true),
        Equals => (Class::Punct, b'=', false),
        DotDot => (Class::Punct, b'.', false),
    };
    Some(Piece {
        span: token.span,
        class,
        ch,
        ends_statement,
    })
}

fn gap(previous: &Piece, next: &Piece) -> Gap {
    if matches!(
        next.ch,
        b',' | b';' | b')' | b']' | b':' | b'(' | b'[' | b'=' | b'.'
    ) || matches!(previous.ch, b'(' | b'[' | b'=' | b'.')
    {
        return Gap::None;
    }
    Gap::One
}

/// The token stream as (kind, text), for the before/after comparison.
fn keys(source: &str, tokens: &[Token]) -> Vec<(u8, String)> {
    tokens
        .iter()
        .filter(|t| t.content != TokenContent::Eof)
        .map(|t| {
            let text = t.span.slice(source);
            if t.is_trivia() {
                (1, comment_key(text))
            } else {
                (0, text.to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(source: &str) -> String {
        format(source).expect("the source should format")
    }

    #[test]
    fn formats_a_messy_chip() {
        let source = "CHIP   Majority{\nIN a,b ,c;\n  OUT out ;\n\n\n\nPARTS:\n  And( a = a , b=b,out=ab );\nOr (a=ab,b=c,out=out) ;\n}";
        assert_eq!(
            fmt(source),
            "CHIP Majority {\n    IN a, b, c;\n    OUT out;\n\n    PARTS:\n    And(a=a, b=b, out=ab);\n    Or(a=ab, b=c, out=out);\n}\n"
        );
    }

    #[test]
    fn is_idempotent_and_keeps_buses() {
        let source = "CHIP Low8 {\n    IN in[16];\n    OUT out[8];\n\n    PARTS:\n    Or16(a=in, b[0..7]=false, b[8..15]=true, out[0..7]=out);\n}\n";
        assert_eq!(fmt(source), source);
        assert_eq!(fmt(&fmt(source)), fmt(source));
    }

    /// Review finding A1: a `//` comment before a `,` or `;` used to swallow
    /// the punctuation, silently changing the chip.
    #[test]
    fn a_comment_before_punctuation_never_swallows_it() {
        let source = "CHIP Pair {\n    IN a // first\n    , b;\n    OUT out;\n    PARTS:\n    And(a=a // left\n        , b=b, out=out // done\n    );\n}\n";
        let formatted = fmt(source);
        assert!(formatted.contains("IN a // first\n"), "{formatted}");
        assert!(formatted.contains(", b;"), "{formatted}");
        assert!(formatted.contains("And(a=a // left\n"), "{formatted}");
        // The chip still means the same thing.
        let before = crate::hdl::parse(source);
        let after = crate::hdl::parse(&formatted);
        assert!(after.diagnostics.is_empty(), "{:?}", after.diagnostics);
        let pins = |p: &crate::hdl::ParsedHdl| {
            let chip = p.chip.as_ref().unwrap();
            (
                chip.inputs
                    .iter()
                    .map(|i| i.name.name.clone())
                    .collect::<Vec<_>>(),
                chip.outputs
                    .iter()
                    .map(|i| i.name.name.clone())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(pins(&before), pins(&after));
        assert_eq!(pins(&after).0, ["a", "b"]);
    }

    #[test]
    fn keeps_every_comment_exactly() {
        let source = "/**\n * Majority of three.\n *   out = 1 when two inputs are 1\n */\n\nCHIP Majority {\n        IN a, b, c;   // inputs\n    OUT out; /* one bit */\n\n    PARTS:\n    // TODO\n\n    /* a block\n       spanning lines */\n        And(a=a, b=b, out=ab);  // first\n}\n";
        let formatted = fmt(source);
        for needle in [
            "/**\n * Majority of three.\n *   out = 1 when two inputs are 1\n */",
            "    IN a, b, c;   // inputs",
            "    OUT out; /* one bit */",
            "    // TODO",
            "    /* a block\n       spanning lines */",
            "    And(a=a, b=b, out=ab);  // first",
        ] {
            assert!(
                formatted.contains(needle),
                "missing {needle:?} in:\n{formatted}"
            );
        }
    }

    #[test]
    fn keeps_line_structure_and_continuation_alignment() {
        let source = "CHIP Wide {\n  IN a,\n     b;\n  OUT out;\n  PARTS:\n  And(a=a,\n      b=b,\n      out=out);\n}\n";
        assert_eq!(
            fmt(source),
            "CHIP Wide {\n    IN a,\n       b;\n    OUT out;\n    PARTS:\n    And(a=a,\n        b=b,\n        out=out);\n}\n"
        );
    }

    #[test]
    fn preserves_crlf_bom_and_tabs_option() {
        let source =
            "\u{FEFF}CHIP T {\r\nIN a;\r\nOUT out;\r\nPARTS:\r\nNot(in=a,out=out);\r\n}\r\n";
        let formatted = fmt(source);
        assert!(formatted.starts_with('\u{FEFF}'));
        assert_eq!(formatted.matches("\r\n").count(), 6);
        assert!(!formatted.replace("\r\n", "").contains('\n'));
        let tabs = format_with_options(
            "CHIP T {\nIN a;\nOUT out;\nPARTS:\nNot(in=a,out=out);\n}\n",
            &FormatOptions {
                use_tabs: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(tabs.contains("\n\tIN a;\n"));
    }

    #[test]
    fn refuses_syntax_errors() {
        assert_eq!(format("CHIP Broken {\n    IN a\n    OUT out;\n}"), None);
        assert_eq!(format("CHIP {"), None);
        assert_eq!(format(""), None);
    }

    /// Whatever the input, formatting either refuses or returns text with the
    /// same tokens, and formatting twice changes nothing more.
    #[test]
    fn never_changes_tokens_on_mutated_input() {
        let seed = "/* head */\nCHIP Mix {\n    IN a, b[4], sel; // ins\n    OUT out[4], z;\n\n    PARTS:\n    Mux(a=a, b=b[0], sel=sel, out=out[0]); // one\n    /* two\n       lines */ Not(in=a, out=z);\n    Or16(a[0..3]=b, b=true, out[0..3]=out);\n}\n";
        let inserts = [
            "//x\n",
            " ",
            "\t",
            "\n",
            "\r\n",
            "/*y*/",
            ",",
            ";",
            " // c",
            "\n\n\n",
            "/* m\n n */",
        ];
        let mut state = 0x2545F4914F6CDD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut formatted_count = 0;
        for _ in 0..4000 {
            let mut text = seed.to_string();
            for _ in 0..(next() % 4 + 1) {
                let at = (next() as usize) % (text.len() + 1);
                if !text.is_char_boundary(at) {
                    continue;
                }
                let insert = inserts[(next() as usize) % inserts.len()];
                text.insert_str(at, insert);
            }
            let Some(formatted) = format(&text) else {
                continue;
            };
            formatted_count += 1;
            let before = crate::hdl::lexer::lex(&text);
            let after = crate::hdl::lexer::lex(&formatted);
            assert_eq!(
                keys(&text, &before.tokens),
                keys(&formatted, &after.tokens),
                "tokens changed for:\n{text}\n---\n{formatted}"
            );
            assert_eq!(
                format(&formatted).as_deref(),
                Some(formatted.as_str()),
                "not idempotent for:\n{text}"
            );
        }
        assert!(
            formatted_count > 500,
            "only {formatted_count} inputs formatted"
        );
    }
}
