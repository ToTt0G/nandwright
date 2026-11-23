//! Shared formatting configuration and comment helpers for `.hdl` and `.tst` formatters.

/// Formatting options shared by `.hdl` and `.tst` formatters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// Spaces per indentation level (ignored when `use_tabs` is true).
    pub tab_size: usize,
    /// Emit `\t` instead of spaces for indentation.
    pub use_tabs: bool,
    /// Soft line-width target before wrapping long pin/connection lists.
    pub max_line_width: usize,
    /// Ensure formatted output terminates with a newline.
    pub trailing_newline: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            use_tabs: false,
            max_line_width: 100,
            trailing_newline: true,
        }
    }
}

impl FormatOptions {
    /// Returns the indentation string for `level` nesting levels.
    pub fn indent(&self, level: usize) -> String {
        if self.use_tabs {
            "\t".repeat(level)
        } else {
            " ".repeat(self.tab_size * level)
        }
    }

    /// Visual column width of `level` indentation levels.
    pub fn indent_width(&self, level: usize) -> usize {
        self.tab_size * level
    }
}

/// Counts line breaks (`\n`) in `slice`.
pub(crate) fn newline_count(slice: &str) -> usize {
    slice.bytes().filter(|&b| b == b'\n').count()
}

/// Re-indents a single-line or multi-line comment verbatim from `raw` so its
/// continuation lines align cleanly at `indent`.
pub(crate) fn format_comment(raw: &str, indent: &str) -> String {
    if !raw.contains('\n') {
        return raw.trim_end_matches([' ', '\t', '\r']).to_string();
    }
    let mut out = String::with_capacity(raw.len());
    for (i, line) in raw.lines().enumerate() {
        let trimmed_end = line.trim_end_matches([' ', '\t', '\r']);
        if i == 0 {
            out.push_str(trimmed_end);
        } else {
            out.push('\n');
            let trimmed = trimmed_end.trim_start_matches([' ', '\t']);
            if trimmed.is_empty() {
                // Keep empty lines inside block comments free of trailing whitespace.
            } else if trimmed.starts_with('*') {
                // Align `*` under the `*` in `/*` or `/**`.
                out.push_str(indent);
                out.push(' ');
                out.push_str(trimmed);
            } else {
                out.push_str(indent);
                out.push_str(trimmed);
            }
        }
    }
    out
}

/// Converts `\n` line endings to `\r\n` when `source` predominantly uses CRLF.
pub(crate) fn apply_line_ending(source: &str, formatted_lf: String) -> String {
    if source.contains("\r\n") {
        formatted_lf.replace('\n', "\r\n")
    } else {
        formatted_lf
    }
}
