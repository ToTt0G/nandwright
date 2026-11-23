//! Document and range formatting for `.hdl` and `.tst` documents.

use lsp_types as lsp;
use lsp_types::Url;
use n2t_core::FormatOptions;

use crate::workspace::{DocKind, Workspace};

fn options_from_lsp(options: &lsp::FormattingOptions) -> FormatOptions {
    FormatOptions {
        tab_size: (options.tab_size as usize).max(1),
        use_tabs: !options.insert_spaces,
        max_line_width: FormatOptions::default().max_line_width,
        trailing_newline: options.insert_final_newline.unwrap_or(true),
    }
}

/// Formats an entire `.hdl` or `.tst` document, returning a minimal edit list
/// (empty if already formatted, `None` if the document has syntax errors).
pub fn document(
    ws: &Workspace,
    uri: &Url,
    options: &lsp::FormattingOptions,
) -> Option<Vec<lsp::TextEdit>> {
    let doc = ws.docs.get(uri)?;
    let opts = options_from_lsp(options);
    let formatted = match doc.kind {
        DocKind::Hdl => n2t_core::hdl::format_with_options(&doc.text, &opts)?,
        DocKind::Tst => n2t_core::tst::format_with_options(&doc.text, &opts)?,
        DocKind::Other => return None,
    };
    Some(minimal_edit(&doc.text, &doc.lines, &formatted))
}

/// Formats the portion of a `.hdl` or `.tst` document overlapping `range`.
///
/// For `.hdl` files, if the formatting edit stays within the requested lines,
/// only those lines are replaced; if the requested range touches a wrapped
/// multi-line section or part call, the edit covers the affected lines cleanly.
pub fn range(
    ws: &Workspace,
    uri: &Url,
    range: lsp::Range,
    options: &lsp::FormattingOptions,
) -> Option<Vec<lsp::TextEdit>> {
    let doc = ws.docs.get(uri)?;
    let opts = options_from_lsp(options);
    let formatted = match doc.kind {
        DocKind::Hdl => n2t_core::hdl::format_with_options(&doc.text, &opts)?,
        DocKind::Tst => n2t_core::tst::format_with_options(&doc.text, &opts)?,
        DocKind::Other => return None,
    };
    if formatted == &*doc.text {
        return Some(Vec::new());
    }

    let orig_lines: Vec<&str> = doc.text.split_inclusive('\n').collect();
    let fmt_lines: Vec<&str> = formatted.split_inclusive('\n').collect();

    // When the formatted output has the same line count as the original,
    // restrict edits strictly to the lines overlapping `range`.
    if orig_lines.len() == fmt_lines.len() {
        let start_line = (range.start.line as usize).min(orig_lines.len());
        let end_line = if range.end.character == 0 && range.end.line > range.start.line {
            (range.end.line as usize).min(orig_lines.len())
        } else {
            (range.end.line as usize + 1).min(orig_lines.len())
        };
        if start_line >= end_line {
            return Some(Vec::new());
        }
        let mut edits = Vec::new();
        let text = doc.text();
        let mut offset = orig_lines[..start_line]
            .iter()
            .map(|l| l.len())
            .sum::<usize>();
        for i in start_line..end_line {
            let orig = orig_lines[i];
            let next = fmt_lines[i];
            if orig != next {
                let span = n2t_core::Span::new(offset, offset + orig.len());
                edits.push(lsp::TextEdit::new(text.range(span), next.to_string()));
            }
            offset += orig.len();
        }
        return Some(edits);
    }

    // When wrapping/unwrapping or blank-line normalization changes line counts,
    // emit the minimal full-document diff so the result stays syntactically valid.
    Some(minimal_edit(&doc.text, &doc.lines, &formatted))
}

/// Computes a single minimal `TextEdit` transforming `original` into `formatted`
/// by trimming identical prefix and suffix slices along UTF-8 char boundaries.
fn minimal_edit(
    original: &str,
    lines: &n2t_core::LineIndex,
    formatted: &str,
) -> Vec<lsp::TextEdit> {
    if original == formatted {
        return Vec::new();
    }

    let orig_bytes = original.as_bytes();
    let fmt_bytes = formatted.as_bytes();

    let mut prefix = 0usize;
    let min_len = orig_bytes.len().min(fmt_bytes.len());
    while prefix < min_len && orig_bytes[prefix] == fmt_bytes[prefix] {
        prefix += 1;
    }
    while prefix > 0 && (!original.is_char_boundary(prefix) || !formatted.is_char_boundary(prefix))
    {
        prefix -= 1;
    }

    let mut orig_end = orig_bytes.len();
    let mut fmt_end = fmt_bytes.len();
    while orig_end > prefix
        && fmt_end > prefix
        && orig_bytes[orig_end - 1] == fmt_bytes[fmt_end - 1]
    {
        orig_end -= 1;
        fmt_end -= 1;
    }
    while orig_end < original.len()
        && (!original.is_char_boundary(orig_end) || !formatted.is_char_boundary(fmt_end))
    {
        orig_end += 1;
        fmt_end += 1;
    }

    let text = crate::convert::Text::new(original, lines);
    let span = n2t_core::Span::new(prefix, orig_end);
    vec![lsp::TextEdit::new(
        text.range(span),
        formatted[prefix..fmt_end].to_string(),
    )]
}
