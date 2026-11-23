//! Formatter for `.tst` test scripts.
//!
//! Like the HDL formatter it is line-preserving (see [`crate::format`]): it
//! fixes indentation inside `repeat`/`while` blocks and the spacing within
//! each line, and never moves a command or a comment to another line.

use crate::diagnostic::Severity;
use crate::format::{Class, FormatOptions, Gap, Piece, comment_key, reformat, same_tokens};
use crate::tst::lexer::{Token, TokenKind};

/// Formats a `.tst` source using the default [`FormatOptions`].
///
/// Returns `None` if the source contains lexical or syntax errors.
pub fn format(source: &str) -> Option<String> {
    format_with_options(source, &FormatOptions::default())
}

/// Formats a `.tst` source with custom [`FormatOptions`].
///
/// Returns `None` if the source has lexical or syntax errors, or if the
/// formatted text would not lex to exactly the same tokens and comments.
pub fn format_with_options(source: &str, options: &FormatOptions) -> Option<String> {
    let parsed = crate::tst::parse(source);
    if parsed
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return None;
    }
    let pieces: Vec<Piece> = parsed.tokens.iter().filter_map(piece).collect();
    let formatted = reformat(source, &pieces, options, &gap);
    let after = crate::tst::lexer::lex(&formatted);
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

/// `ch` values for tokens that are not single punctuation characters.
const PERCENT: u8 = b'%';
const RELATION: u8 = b'<';

fn piece(token: &Token) -> Option<Piece> {
    use TokenKind::*;
    let (class, ch, ends_statement) = match &token.kind {
        Eof => return None,
        Comment(_) | DocComment(_) => (Class::Comment, 0, false),
        Word(_) | String { .. } => (Class::Word, 0, false),
        Percent { .. } => (Class::Word, PERCENT, false),
        LeftBrace => (Class::Open, b'{', true),
        RightBrace => (Class::Close, b'}', true),
        LeftBracket => (Class::Punct, b'[', false),
        RightBracket => (Class::Punct, b']', false),
        Comma => (Class::Punct, b',', true),
        Semicolon => (Class::Punct, b';', true),
        Bang => (Class::Punct, b'!', true),
        Equals | Less | Greater | LessEqual | GreaterEqual | NotEqual => {
            (Class::Punct, RELATION, false)
        }
    };
    Some(Piece {
        span: token.span,
        class,
        ch,
        ends_statement,
    })
}

fn gap(previous: &Piece, next: &Piece) -> Gap {
    if matches!(next.ch, b',' | b';' | b'!' | b']') || previous.ch == b'[' {
        return Gap::None;
    }
    // `RAM16K[3]` and `a%B3.1.3` are written without a space, `set in %B0101`
    // with one: keep what the script has.
    if next.ch == b'[' || next.ch == PERCENT || previous.ch == b']' {
        return Gap::Keep;
    }
    Gap::One
}

fn keys(source: &str, tokens: &[Token]) -> Vec<(u8, String)> {
    tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
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
    fn formats_a_messy_script() {
        let source = "load Majority.hdl ,\noutput-file Majority.out,\n  compare-to Majority.cmp ,\noutput-list a%B3.1.3 b%B3.1.3   out%B3.1.3 ;\n\n\n\nset a 0 ,set b 1,  eval ,output ;\nrepeat 3 {\ntick , tock ,\n   output;\n}\n";
        assert_eq!(
            fmt(source),
            "load Majority.hdl,\noutput-file Majority.out,\ncompare-to Majority.cmp,\noutput-list a%B3.1.3 b%B3.1.3 out%B3.1.3;\n\nset a 0, set b 1, eval, output;\nrepeat 3 {\n    tick, tock,\n    output;\n}\n"
        );
    }

    #[test]
    fn keeps_comments_values_and_state_access() {
        let source = "// header\nload Counter.hdl;   // the chip\n/* block\n   comment */\nset in %B0101, set RAM16K[3] -1, set PC[] 5;\nwhile out <> 75 {\n  eval, // wait\n}\noutput-list time%S1.4.1\n            in%D1.6.1;\n";
        let formatted = fmt(source);
        for needle in [
            "// header\n",
            "load Counter.hdl;   // the chip\n",
            "/* block\n   comment */\n",
            "set in %B0101, set RAM16K[3] -1, set PC[] 5;\n",
            "while out <> 75 {\n    eval, // wait\n}\n",
            "output-list time%S1.4.1\n            in%D1.6.1;\n",
        ] {
            assert!(
                formatted.contains(needle),
                "missing {needle:?} in:\n{formatted}"
            );
        }
        assert_eq!(fmt(&formatted), formatted);
    }

    #[test]
    fn refuses_syntax_errors() {
        assert_eq!(format("repeat {\n"), None);
        assert_eq!(format("load \"unterminated"), None);
    }

    #[test]
    fn never_changes_tokens_on_mutated_input() {
        let seed = "// t\nload Mix.hdl, output-file Mix.out, compare-to Mix.cmp,\noutput-list in%B1.4.1 out%D2.6.2 RAM8[2]%X1.4.1;\nset in %B0101, eval, output;\nrepeat 2 { tick, tock; } /* c\n d */ while out <> 3 { eval, }\necho \"a b\"; breakpoint out 4; clear-breakpoints;\n";
        let inserts = [
            "//x\n", " ", "\t", "\n", "\r\n", "/*y*/", ",", ";", " // c", "\n\n\n", "!",
        ];
        let mut state = 0x9E3779B97F4A7C15u64;
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
                text.insert_str(at, inserts[(next() as usize) % inserts.len()]);
            }
            let Some(formatted) = format(&text) else {
                continue;
            };
            formatted_count += 1;
            let before = crate::tst::lexer::lex(&text);
            let after = crate::tst::lexer::lex(&formatted);
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
