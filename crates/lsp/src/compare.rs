//! `.out` files compared with their sibling `.cmp` file.
//!
//! The rule is the official one, restated here so the language server does
//! not depend on the simulator: the files are compared line by line, a line
//! matches when it has the same length as the expected line and every
//! character is equal, except that `*` in the compare file matches any one
//! character. Line terminators (`\n` or `\r\n`) are not part of a line, and
//! a final line terminator does not start another line.

use lsp_types::Url;
use n2t_core::host::{path, read_text};
use n2t_core::{Diagnostic, DiagnosticCode, Span};

use crate::workspace::Workspace;

/// At most this many differing lines are reported.
const MAX_DIAGNOSTICS: usize = 100;

/// The lines of a text with their byte spans (without terminators).
pub fn lines(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            let end = if i > start && text.as_bytes().get(i - 1) == Some(&b'\r') {
                i - 1
            } else {
                i
            };
            out.push(Span::new(start, end));
            start = i + 1;
        }
    }
    if start < text.len() {
        let end = if text.ends_with('\r') {
            text.len() - 1
        } else {
            text.len()
        };
        out.push(Span::new(start, end.max(start)));
    }
    out
}

/// The index of the first character where `actual` fails to match
/// `expected`, or `None` if the line matches.
pub fn first_difference(expected: &str, actual: &str) -> Option<usize> {
    let e: Vec<char> = expected.chars().collect();
    let a: Vec<char> = actual.chars().collect();
    e.iter()
        .zip(&a)
        .position(|(e, a)| *e != '*' && e != a)
        .or_else(|| (e.len() != a.len()).then(|| e.len().min(a.len())))
}

/// The `|`-separated cell around character `index` of `line`: its byte
/// range, and its position among the cells.
fn cell(line: &str, index: usize) -> (usize, usize, usize) {
    let byte = line
        .char_indices()
        .nth(index)
        .map_or(line.len(), |(b, _)| b);
    let start = line
        .get(..byte)
        .and_then(|s| s.rfind('|'))
        .map_or(0, |i| i + 1);
    let end = line
        .get(byte..)
        .and_then(|s| s.find('|'))
        .map_or(line.len(), |i| byte + i);
    let column = line.get(..start).map_or(0, |s| s.matches('|').count());
    (start, end.max(start), column)
}

fn cell_text(line: &str, column: usize) -> Option<&str> {
    line.split('|').nth(column).map(str::trim)
}

/// Diagnostics for `out` (the text of an `.out` file) compared with `cmp`.
pub fn compare(out: &str, cmp: &str, cmp_name: &str) -> Vec<Diagnostic> {
    let out_lines = lines(out);
    let cmp_lines = lines(cmp);
    let header = cmp_lines.first().and_then(|s| cmp.get(s.start..s.end));
    let mut diagnostics = Vec::new();
    for (i, span) in out_lines.iter().enumerate() {
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            break;
        }
        let actual = out.get(span.start..span.end).unwrap_or("");
        let Some(expected) = cmp_lines.get(i).and_then(|s| cmp.get(s.start..s.end)) else {
            if actual.trim().is_empty() {
                continue;
            }
            diagnostics.push(Diagnostic::error(
                DiagnosticCode::Other("out-extra-line"),
                format!(
                    "line {} is not in `{cmp_name}`, which has {} line{}",
                    i + 1,
                    cmp_lines.len(),
                    if cmp_lines.len() == 1 { "" } else { "s" }
                ),
                *span,
            ));
            continue;
        };
        let Some(index) = first_difference(expected, actual) else {
            continue;
        };
        let (start, end, column) = cell(actual, index);
        let found = cell_text(actual, column).unwrap_or("");
        let wanted = cell_text(expected, column).unwrap_or("");
        let name = header
            .filter(|_| i > 0)
            .and_then(|h| cell_text(h, column))
            .filter(|n| !n.is_empty())
            .map(|n| format!(" (column `{n}`)"))
            .unwrap_or_default();
        let message = if wanted != found {
            format!(
                "line {} differs from `{cmp_name}`{name}: expected `{wanted}`, found `{found}`",
                i + 1
            )
        } else {
            // Same cell text, different spacing or length.
            format!(
                "line {} differs from `{cmp_name}` at character {}: expected `{expected}`, \
                 found `{actual}`",
                i + 1,
                index + 1
            )
        };
        let range = if start < end {
            Span::new(span.start + start, span.start + end)
        } else {
            Span::new(span.start + start, span.end.max(span.start + start))
        };
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::Other("out-differs"),
            message,
            range,
        ));
    }
    let expected_lines = cmp_lines
        .iter()
        .rposition(|s| {
            cmp.get(s.start..s.end)
                .is_some_and(|l| !l.trim().is_empty())
        })
        .map_or(0, |i| i + 1);
    if out_lines.len() < expected_lines && diagnostics.len() < MAX_DIAGNOSTICS {
        let last = out_lines.last().copied().unwrap_or_default();
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::Other("out-missing-lines"),
            format!(
                "the output stops after {} line{}; `{cmp_name}` expects {expected_lines}",
                out_lines.len(),
                if out_lines.len() == 1 { "" } else { "s" }
            ),
            last,
        ));
    }
    diagnostics
}

/// Diagnostics for an `.out` document: a comparison with the `.cmp` file of
/// the same name in its folder, if there is one.
pub fn diagnostics(ws: &mut Workspace, uri: &Url) -> Vec<Diagnostic> {
    let Some(doc) = ws.docs.get(uri) else {
        return Vec::new();
    };
    let Some(out_path) = &doc.path else {
        return Vec::new();
    };
    let cmp_name = format!("{}.cmp", doc.stem);
    let cmp_path = path::join(path::dir(out_path), &cmp_name);
    let cmp = match ws
        .docs
        .values()
        .find(|d| d.path.as_deref() == Some(&cmp_path))
    {
        Some(open) => open.text.to_string(),
        None => match read_text(ws.host.as_ref(), &cmp_path) {
            Ok(file) => file.text,
            Err(_) => return Vec::new(),
        },
    };
    compare(&doc.text, &cmp, &cmp_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_official_rule() {
        assert_eq!(first_difference("| 1 |", "| 1 |"), None);
        assert_eq!(
            first_difference("| * |", "| 0 |"),
            None,
            "* matches any one character"
        );
        assert_eq!(first_difference("|*|", "|10|"), Some(2), "but only one");
        assert_eq!(first_difference("| 1 |", "| 0 |"), Some(2));
        assert_eq!(
            first_difference("| 1 |", "| 1 | "),
            Some(5),
            "length counts"
        );
        assert_eq!(first_difference("|1|", "|1"), Some(2));
        // `*` in the output file is an ordinary character.
        assert_eq!(first_difference("| 1 |", "| * |"), Some(2));
        let spans = lines("a\r\nb\n\nc");
        assert_eq!(spans.len(), 4);
        assert_eq!(lines("a\n").len(), 1, "a final newline ends the last line");
    }

    #[test]
    fn reports_the_first_differing_cell() {
        let cmp = "|  a  |  b  | out |\n|  0  |  0  |  1  |\n|  0  |  1  |  0  |\n";
        let out = "|  a  |  b  | out |\n|  0  |  0  |  1  |\n|  0  |  1  |  1  |\n";
        let d = compare(out, cmp, "Gate.cmp");
        assert_eq!(d.len(), 1);
        let span = d[0].span.unwrap();
        assert_eq!(&out[span.start..span.end], "  1  ");
        assert!(
            d[0].message.contains("expected `0`, found `1`"),
            "{}",
            d[0].message
        );
        assert!(d[0].message.contains("column `out`"), "{}", d[0].message);
    }
}
