//! Comment-preserving formatter for `.tst` test scripts.
//!
//! Normalizes indentation (including inside `repeat` and `while` blocks),
//! whitespace around operators, punctuation, and format specifiers, and
//! collapses excessive blank lines while preserving intentional command
//! grouping on lines and every comment verbatim.

use crate::diagnostic::Severity;
use crate::format::{FormatOptions, apply_line_ending, format_comment, newline_count};
use crate::tst::lexer::TokenKind;

/// Formats a `.tst` script using the default [`FormatOptions`].
///
/// Returns `None` if the script contains lexical or syntax errors.
pub fn format(source: &str) -> Option<String> {
    format_with_options(source, &FormatOptions::default())
}

/// Formats a `.tst` script with custom [`FormatOptions`].
///
/// Returns `None` if the script contains lexical or syntax errors.
pub fn format_with_options(source: &str, options: &FormatOptions) -> Option<String> {
    let parsed = crate::tst::parse(source);
    if parsed
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return None;
    }

    let mut out = String::with_capacity(source.len() + 64);
    let mut indent_level: usize = 0;
    let mut at_line_start = true;
    let mut force_newline = false;
    let mut just_opened_brace = false;
    let mut prev_kind: Option<&TokenKind> = None;
    let mut last_end = crate::source::bom_len(source);

    for tok in &parsed.tokens {
        if tok.kind == TokenKind::Eof {
            break;
        }
        let gap = if tok.span.start > last_end {
            &source[last_end..tok.span.start]
        } else {
            ""
        };
        let newlines = newline_count(gap);
        let raw = tok.span.slice(source);

        if tok.is_trivia() {
            let is_line = raw.starts_with("//");
            if !at_line_start && newlines == 0 {
                let ind = options.indent(indent_level);
                out.push(' ');
                out.push_str(&format_comment(raw, &ind));
                if is_line || raw.contains('\n') {
                    force_newline = true;
                }
            } else {
                if !out.is_empty() {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if newlines >= 2 && !just_opened_brace && !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                }
                let ind = options.indent(indent_level);
                out.push_str(&ind);
                out.push_str(&format_comment(raw, &ind));
                at_line_start = false;
                force_newline = true;
                just_opened_brace = false;
            }
            last_end = tok.span.end;
            continue;
        }

        match &tok.kind {
            TokenKind::RightBrace => {
                indent_level = indent_level.saturating_sub(1);
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&options.indent(indent_level));
                out.push('}');
                at_line_start = false;
                force_newline = true;
                just_opened_brace = false;
                prev_kind = Some(&tok.kind);
                last_end = tok.span.end;
                continue;
            }
            TokenKind::LeftBrace => {
                if at_line_start {
                    out.push_str(&options.indent(indent_level));
                } else {
                    out.push(' ');
                }
                out.push('{');
                indent_level += 1;
                at_line_start = false;
                force_newline = true;
                just_opened_brace = true;
                prev_kind = Some(&tok.kind);
                last_end = tok.span.end;
                continue;
            }
            _ => {}
        }

        let start_new_line = force_newline || newlines > 0;
        if start_new_line {
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                if newlines >= 2 && !just_opened_brace && !out.ends_with("\n\n") {
                    out.push('\n');
                }
            }
            out.push_str(&options.indent(indent_level));
            force_newline = false;
            just_opened_brace = false;
        } else if at_line_start {
            out.push_str(&options.indent(indent_level));
        } else if needs_space(prev_kind, &tok.kind) {
            out.push(' ');
        }

        out.push_str(raw);
        at_line_start = false;
        prev_kind = Some(&tok.kind);
        last_end = tok.span.end;
    }

    if !out.is_empty() && options.trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }

    Some(apply_line_ending(source, out))
}

fn needs_space(prev: Option<&TokenKind>, curr: &TokenKind) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    if matches!(
        curr,
        TokenKind::Comma
            | TokenKind::Semicolon
            | TokenKind::Bang
            | TokenKind::LeftBracket
            | TokenKind::RightBracket
    ) {
        return false;
    }
    if matches!(prev, TokenKind::LeftBracket) {
        return false;
    }
    // Output format specifiers (`a%B3.1.3`, `RAM16K[0]%D1.7.1`) attach directly
    // to their variable; radix integer literals (`%B101`, `%X00FF`) take a space.
    if matches!(curr, TokenKind::Percent { body, .. } if body.contains('.'))
        && matches!(prev, TokenKind::Word(_) | TokenKind::RightBracket)
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_messy_tst_script() {
        let input = "\
// Header
load   And.hdl ,
output-file And.out ,
compare-to   And.cmp,
output-list   a %B3.1.3   b%B3.1.3   out%B3.1.3 ;


set a   0, set b   1,   eval ,output ;
repeat 10{
tick,   tock , output ;
}
";
        let expected = "\
// Header
load And.hdl,
output-file And.out,
compare-to And.cmp,
output-list a%B3.1.3 b%B3.1.3 out%B3.1.3;

set a 0, set b 1, eval, output;
repeat 10 {
    tick, tock, output;
}
";
        let formatted = format(input).expect("valid tst should format");
        assert_eq!(formatted, expected);
        assert_eq!(format(&formatted).as_deref(), Some(expected));
    }

    #[test]
    fn formats_radix_literals_and_while_loops() {
        let input = "set x %B1010, set y %X00FF, while time<>%S\"6\"{tick,tock;}";
        let expected = "\
set x %B1010, set y %X00FF, while time <> %S\"6\" {
    tick, tock;
}
";
        let formatted = format(input).unwrap();
        assert_eq!(formatted, expected);
        assert_eq!(format(&formatted).as_deref(), Some(expected));
    }

    #[test]
    fn refuses_to_format_syntax_errors() {
        assert!(format("load And.hdl\noutput-file And.out,").is_none());
    }

    #[test]
    fn formats_all_sample_project_tst_files_idempotently() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/projects");
        let mut count = 0;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().map(Result::unwrap) {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "tst") {
                    count += 1;
                    let src = std::fs::read_to_string(&path).unwrap();
                    let once = format(&src)
                        .unwrap_or_else(|| panic!("failed to format {}", path.display()));
                    let twice = format(&once)
                        .unwrap_or_else(|| panic!("failed second format {}", path.display()));
                    assert_eq!(once, twice, "not idempotent for {}", path.display());
                    let reparsed = crate::tst::parse(&once);
                    assert!(
                        reparsed.diagnostics.is_empty(),
                        "formatted {} had diagnostics: {:?}",
                        path.display(),
                        reparsed.diagnostics
                    );
                }
            }
        }
        assert!(count >= 30, "expected >= 30 sample tst files, got {count}");
    }
}
