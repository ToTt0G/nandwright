//! The formatting engine shared by every language.
//!
//! The formatter is **line-preserving**: a token never moves to another line.
//! It only changes whitespace: indentation, the spacing between tokens on a
//! line, trailing whitespace, runs of blank lines and line endings. That makes
//! the classic formatter bug impossible by construction: a `,` or `;` can
//! never be carried onto the end of a `//` comment, because nothing is ever
//! carried anywhere.
//!
//! On top of that, every language adapter re-lexes the result and compares the
//! token stream with the original ([`same_tokens`]). If they differ in any
//! way, formatting is refused and the file is left alone. A formatter must
//! never change what a file means.

use crate::source::Span;

/// Formatting options shared by all formatters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// Spaces per indentation level (ignored when `use_tabs` is true).
    pub tab_size: usize,
    /// Emit `\t` instead of spaces for indentation.
    pub use_tabs: bool,
    /// Ensure formatted output terminates with a newline.
    pub trailing_newline: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            tab_size: 4,
            use_tabs: false,
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
            " ".repeat(self.tab_size.max(1) * level)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// A keyword, name, number or string.
    Word,
    /// `{`: increases the indentation of the lines that follow.
    Open,
    /// `}`.
    Close,
    /// Any other punctuation; `ch` tells which.
    Punct,
    /// A line or block comment.
    Comment,
}

/// One token as the engine sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Piece {
    pub span: Span,
    pub class: Class,
    /// The first byte of a punctuation token (`0` for other classes).
    pub ch: u8,
    /// Whether this token ends a statement: the next line is then a new
    /// statement and not a continuation of this one.
    pub ends_statement: bool,
}

impl Piece {
    fn is_comment(&self) -> bool {
        self.class == Class::Comment
    }
}

/// The spacing between two tokens on the same line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gap {
    None,
    One,
    /// One space if the source had whitespace there, otherwise none.
    Keep,
}

/// Re-indents and re-spaces `source`. `gap` decides the spacing between two
/// adjacent code tokens.
pub(crate) fn reformat(
    source: &str,
    pieces: &[Piece],
    options: &FormatOptions,
    gap: &dyn Fn(&Piece, &Piece) -> Gap,
) -> String {
    let bom = if source.starts_with('\u{FEFF}') {
        "\u{FEFF}"
    } else {
        ""
    };
    let lines = LineStarts::new(source);
    let tab = options.tab_size.max(1);
    let mut out = String::with_capacity(source.len() + 64);
    out.push_str(bom);

    let mut depth: usize = 0;
    // The first line of the statement in progress: its indentation in the
    // source (a display width) and in the output (the string written).
    let mut statement: Option<(usize, String)> = None;
    let mut previous_end_line: Option<usize> = None;
    let mut i = 0;
    while i < pieces.len() {
        // A group is the run of tokens that share an output line: a token
        // joins the group if it starts on the line the previous one ends on.
        let start = i;
        let mut end_line = lines.line_of(
            pieces[i]
                .span
                .end
                .saturating_sub(1)
                .max(pieces[i].span.start),
        );
        i += 1;
        while i < pieces.len() && lines.line_of(pieces[i].span.start) == end_line {
            end_line = lines.line_of(
                pieces[i]
                    .span
                    .end
                    .saturating_sub(1)
                    .max(pieces[i].span.start),
            );
            i += 1;
        }
        let group = &pieces[start..i];
        let first = &group[0];
        let start_line = lines.line_of(first.span.start);

        if let Some(previous) = previous_end_line
            && start_line > previous + 1
        {
            out.push('\n');
        }

        let source_indent = lines.indent_width(source, start_line, tab);
        let level = if first.class == Class::Close {
            depth.saturating_sub(1)
        } else {
            depth
        };
        let mut indent = options.indent(level);
        if let Some((base, base_indent)) = &statement
            && first.class != Class::Close
        {
            // A continuation line keeps its alignment relative to the line
            // that started the statement. Measuring from that line's output
            // indentation keeps formatting idempotent.
            let extra = if source_indent > *base {
                source_indent - *base
            } else {
                tab
            };
            indent = format!("{base_indent}{}", " ".repeat(extra));
        }
        out.push_str(&indent);

        for (k, piece) in group.iter().enumerate() {
            if k > 0 {
                let previous = &group[k - 1];
                let between = &source[previous.span.end..piece.span.start];
                let had_space = !between.is_empty();
                let spacing: &str = if piece.is_comment() {
                    // Keep the student's alignment of trailing comments.
                    if had_space { between } else { " " }
                } else if previous.is_comment() {
                    if had_space { " " } else { "" }
                } else {
                    match gap(previous, piece) {
                        Gap::None => "",
                        Gap::One => " ",
                        Gap::Keep => {
                            if had_space {
                                " "
                            } else {
                                ""
                            }
                        }
                    }
                };
                out.push_str(spacing);
            }
            let text = piece.span.slice(source);
            if piece.class == Class::Comment {
                let old_column = lines.column_width(source, piece.span.start, tab);
                let new_column = last_line_width(&out, tab);
                push_comment(&mut out, text, old_column, new_column, tab);
            } else {
                out.push_str(text);
            }

            match piece.class {
                Class::Open => depth += 1,
                Class::Close => depth = depth.saturating_sub(1),
                _ => {}
            }
            if !piece.is_comment() {
                if piece.ends_statement {
                    statement = None;
                } else if statement.is_none() {
                    statement = Some((source_indent, indent.clone()));
                }
            }
        }
        out.push('\n');
        previous_end_line = Some(end_line);
    }

    if !options.trailing_newline {
        while out.ends_with('\n') {
            out.pop();
        }
    }
    if out == bom {
        // An empty file (or only whitespace) stays empty.
        return bom.to_string();
    }
    apply_line_ending(source, out)
}

/// The display width of the last line of `text`.
fn last_line_width(text: &str, tab: usize) -> usize {
    let line = text.rsplit('\n').next().unwrap_or("");
    line.chars()
        .map(|c| match c {
            '\t' => tab,
            '\u{FEFF}' => 0,
            _ => 1,
        })
        .sum()
}

/// Writes a comment. A block comment's continuation lines are shifted by as
/// much as its first line moved, so the comment keeps its shape. Only leading
/// whitespace is adjusted; the comment's text is never touched.
fn push_comment(out: &mut String, text: &str, old_column: usize, new_column: usize, tab: usize) {
    for (n, line) in text.split('\n').enumerate() {
        let line = line.trim_end_matches(['\r', ' ', '\t']);
        if n == 0 {
            out.push_str(line);
            continue;
        }
        out.push('\n');
        let body = line.trim_start_matches([' ', '\t']);
        if body.is_empty() {
            continue;
        }
        let leading = &line[..line.len() - body.len()];
        let width: usize = leading
            .chars()
            .map(|c| if c == '\t' { tab } else { 1 })
            .sum();
        let shifted = (width + new_column).saturating_sub(old_column);
        out.push_str(&" ".repeat(shifted));
        out.push_str(body);
    }
}

/// Converts `\n` to `\r\n` when most of the source's line breaks are CRLF.
fn apply_line_ending(source: &str, formatted_lf: String) -> String {
    let crlf = source.matches("\r\n").count();
    let total = source.matches('\n').count();
    if crlf > 0 && crlf * 2 >= total {
        formatted_lf.replace('\n', "\r\n")
    } else {
        formatted_lf
    }
}

/// A comment's text with the whitespace a formatter may change removed.
pub(crate) fn comment_key(text: &str) -> String {
    text.split('\n')
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether two token streams are the same. Each token is a kind and its text
/// (for comments, [`comment_key`] of the text).
pub(crate) fn same_tokens(before: &[(u8, String)], after: &[(u8, String)]) -> bool {
    before == after
}

struct LineStarts {
    starts: Vec<usize>,
}

impl LineStarts {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { starts }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset) - 1
    }

    /// The display width of the whitespace at the start of a line.
    fn indent_width(&self, source: &str, line: usize, tab: usize) -> usize {
        let start = self.starts[line];
        source[start..]
            .chars()
            .take_while(|&c| c == ' ' || c == '\t' || c == '\u{FEFF}')
            .map(|c| match c {
                '\t' => tab,
                ' ' => 1,
                _ => 0,
            })
            .sum()
    }

    /// The display column of an offset within its line.
    fn column_width(&self, source: &str, offset: usize, tab: usize) -> usize {
        let start = self.starts[self.line_of(offset)];
        source[start..offset]
            .chars()
            .map(|c| match c {
                '\t' => tab,
                '\u{FEFF}' => 0,
                _ => 1,
            })
            .sum()
    }
}
