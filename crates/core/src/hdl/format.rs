//! Comment-preserving formatter for `.hdl` chip files.
//!
//! Formats syntactically valid `.hdl` sources according to the canonical
//! Nand to Tetris style:
//!
//! ```hdl
//! CHIP And {
//!     IN a, b;
//!     OUT out;
//!
//!     PARTS: }
//! ```
//!
//! All comments (line, block, and doc comments) and intentional single blank
//! lines between part groups are preserved. Syntactically invalid sources are
//! left untouched (`None`) so formatting never destroys partial edits.

use crate::diagnostic::Severity;
use crate::format::{FormatOptions, apply_line_ending, format_comment, newline_count};
use crate::hdl::lexer::{Keyword, Token, TokenContent};

/// Formats an `.hdl` source using the default [`FormatOptions`].
///
/// Returns `None` if the source contains lexical or syntax errors.
pub fn format(source: &str) -> Option<String> {
    format_with_options(source, &FormatOptions::default())
}

/// Formats an `.hdl` source with custom [`FormatOptions`].
///
/// Returns `None` if the source contains lexical or syntax errors.
/// Warnings (such as non-portable underscores in identifiers) do not prevent formatting.
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

    let mut fmt = HdlFormatter::new(source, &parsed.tokens, options);
    let formatted = fmt.format_file()?;
    Some(apply_line_ending(source, formatted))
}

#[derive(Debug, Clone)]
struct TriviaItem<'a> {
    raw: &'a str,
    newlines_before: usize,
}

#[derive(Debug, Clone)]
struct ListEntry<'a> {
    leading: Vec<TriviaItem<'a>>,
    text: String,
    inline_comments: Vec<&'a str>,
    trailing: Option<&'a str>,
    had_blank_before: bool,
}

struct HdlFormatter<'a> {
    source: &'a str,
    tokens: &'a [Token],
    pos: usize,
    last_end: usize,
    options: &'a FormatOptions,
    out: String,
}

impl<'a> HdlFormatter<'a> {
    fn new(source: &'a str, tokens: &'a [Token], options: &'a FormatOptions) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            last_end: crate::source::bom_len(source),
            options,
            out: String::with_capacity(source.len() + 64),
        }
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn peek_sig(&self) -> Option<&'a Token> {
        let mut i = self.pos;
        while let Some(tok) = self.tokens.get(i) {
            if !tok.is_trivia() {
                return Some(tok);
            }
            i += 1;
        }
        None
    }

    fn gap_newlines(&self, next_start: usize) -> usize {
        if next_start <= self.last_end {
            0
        } else {
            newline_count(&self.source[self.last_end..next_start])
        }
    }

    /// Consumes a trailing same-line comment (if one immediately follows the
    /// previous token on the same line) and returns its raw slice.
    fn take_same_line_comment(&mut self) -> Option<&'a str> {
        let tok = self.peek()?;
        if !tok.is_trivia() {
            return None;
        }
        if self.gap_newlines(tok.span.start) > 0 {
            return None;
        }
        let raw = tok.span.slice(self.source);
        // Only treat block comments as trailing line comments if they end the line
        // (i.e. followed by a newline or EOF) or are line comments.
        let is_line = raw.starts_with("//");
        if !is_line {
            let next_start = self
                .tokens
                .get(self.pos + 1)
                .map_or(self.source.len(), |t| t.span.start);
            let ends_line = next_start >= self.source.len()
                || newline_count(&self.source[tok.span.end..next_start]) > 0;
            if !ends_line {
                return None;
            }
        }
        self.pos += 1;
        self.last_end = tok.span.end;
        Some(raw)
    }

    /// Consumes all trivia tokens up to the next non-trivia token, returning
    /// them alongside the newline gap before the next non-trivia token.
    fn take_leading_trivia(&mut self) -> (Vec<TriviaItem<'a>>, usize) {
        let mut items = Vec::new();
        while let Some(tok) = self.peek() {
            if !tok.is_trivia() {
                break;
            }
            let newlines_before = self.gap_newlines(tok.span.start);
            let raw = tok.span.slice(self.source);
            items.push(TriviaItem {
                raw,
                newlines_before,
            });
            self.pos += 1;
            self.last_end = tok.span.end;
        }
        let gap_after = self
            .peek()
            .map_or(0, |t| self.gap_newlines(t.span.start));
        (items, gap_after)
    }

    /// Consumes any inline trivia right before a mid-construct token, collecting
    /// raw comment text so no comment is ever dropped.
    fn take_inline_trivia(&mut self, sink: &mut Vec<&'a str>) {
        while let Some(tok) = self.peek() {
            if !tok.is_trivia() {
                break;
            }
            sink.push(tok.span.slice(self.source));
            self.pos += 1;
            self.last_end = tok.span.end;
        }
    }

    fn bump_sig(&mut self) -> Option<&'a Token> {
        let tok = self.peek()?;
        debug_assert!(!tok.is_trivia());
        self.pos += 1;
        self.last_end = tok.span.end;
        Some(tok)
    }

    fn emit_trailing_comment(&mut self, raw: &'a str, indent: &str) {
        self.out.push(' ');
        self.out.push_str(&format_comment(raw, indent));
    }

    fn emit_trivia_block(
        &mut self,
        trivia: &[TriviaItem<'a>],
        indent: &str,
        allow_initial_blank: bool,
        force_initial_blank: bool,
    ) {
        let mut need_blank = force_initial_blank;
        for (i, item) in trivia.iter().enumerate() {
            let had_blank = item.newlines_before >= 2;
            if i == 0 {
                if !self.out.is_empty() && (need_blank || (allow_initial_blank && had_blank)) {
                    self.ensure_blank_line();
                }
            } else if had_blank {
                self.ensure_blank_line();
            }
            need_blank = false;
            self.out.push_str(indent);
            self.out.push_str(&format_comment(item.raw, indent));
            self.out.push('\n');
        }
    }

    fn ensure_blank_line(&mut self) {
        if self.out.is_empty() {
            return;
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn format_file(&mut self) -> Option<String> {
        let ind0 = self.options.indent(0);
        let ind1 = self.options.indent(1);

        // Leading file / doc comments before `CHIP`.
        let (leading, gap_before_chip) = self.take_leading_trivia();
        self.emit_trivia_block(&leading, &ind0, false, false);
        if !leading.is_empty() && gap_before_chip >= 2 {
            self.ensure_blank_line();
        }

        // `CHIP <Name> {`
        let chip_kw = self.bump_sig()?;
        if chip_kw.content != TokenContent::Keyword(Keyword::Chip) {
            return None;
        }
        let mut header_inline = Vec::new();
        self.take_inline_trivia(&mut header_inline);
        let name_tok = self.bump_sig()?;
        let chip_name = match &name_tok.content {
            TokenContent::Ident(n) => n.as_str(),
            _ => return None,
        };
        self.take_inline_trivia(&mut header_inline);
        let lbrace = self.bump_sig()?;
        if lbrace.content != TokenContent::LeftBrace {
            return None;
        }

        self.out.push_str("CHIP ");
        self.out.push_str(chip_name);
        for c in &header_inline {
            self.out.push(' ');
            self.out.push_str(&format_comment(c, &ind0));
        }
        self.out.push_str(" {");
        if let Some(comment) = self.take_same_line_comment() {
            self.emit_trailing_comment(comment, &ind0);
        }
        self.out.push('\n');

        // Body sections inside `{ ... }`.
        let mut had_pin_section = false;
        let mut had_any_section = false;

        loop {
            let (sec_trivia, gap_before_sec) = self.take_leading_trivia();
            let Some(next) = self.peek() else {
                return None;
            };
            match &next.content {
                TokenContent::Keyword(Keyword::In) => {
                    self.emit_trivia_block(&sec_trivia, &ind1, had_any_section, false);
                    if !sec_trivia.is_empty() && gap_before_sec >= 2 {
                        self.ensure_blank_line();
                    }
                    self.format_pin_section("IN")?;
                    had_pin_section = true;
                    had_any_section = true;
                }
                TokenContent::Keyword(Keyword::Out) => {
                    self.emit_trivia_block(&sec_trivia, &ind1, had_any_section, false);
                    if !sec_trivia.is_empty() && gap_before_sec >= 2 {
                        self.ensure_blank_line();
                    }
                    self.format_pin_section("OUT")?;
                    had_pin_section = true;
                    had_any_section = true;
                }
                TokenContent::Keyword(Keyword::Parts) => {
                    if sec_trivia.is_empty() {
                        if had_pin_section {
                            self.ensure_blank_line();
                        }
                    } else {
                        self.emit_trivia_block(&sec_trivia, &ind1, had_any_section, had_pin_section);
                        if gap_before_sec >= 2 {
                            self.ensure_blank_line();
                        }
                    }
                    self.format_parts_section()?;
                    had_any_section = true;
                }
                TokenContent::Keyword(Keyword::Builtin) => {
                    if sec_trivia.is_empty() {
                        if had_pin_section {
                            self.ensure_blank_line();
                        }
                    } else {
                        self.emit_trivia_block(&sec_trivia, &ind1, had_any_section, had_pin_section);
                        if gap_before_sec >= 2 {
                            self.ensure_blank_line();
                        }
                    }
                    self.format_builtin_section()?;
                    had_any_section = true;
                }
                TokenContent::Keyword(Keyword::Clocked) => {
                    self.emit_trivia_block(&sec_trivia, &ind1, had_any_section, false);
                    if !sec_trivia.is_empty() && gap_before_sec >= 2 {
                        self.ensure_blank_line();
                    }
                    self.format_clocked_section()?;
                    had_any_section = true;
                }
                TokenContent::RightBrace => {
                    self.emit_trivia_block(&sec_trivia, &ind1, had_any_section, false);
                    self.bump_sig()?;
                    self.out.push('}');
                    if let Some(comment) = self.take_same_line_comment() {
                        self.emit_trailing_comment(comment, &ind0);
                    }
                    self.out.push('\n');
                    break;
                }
                _ => return None,
            }
        }

        // Trailing comments after `}` before EOF.
        let (trailing, _) = self.take_leading_trivia();
        self.emit_trivia_block(&trailing, &ind0, true, false);

        if !self.options.trailing_newline && self.out.ends_with('\n') {
            self.out.pop();
        }
        Some(std::mem::take(&mut self.out))
    }

    fn format_pin_section(&mut self, keyword: &str) -> Option<()> {
        let ind1 = self.options.indent(1);
        let ind2 = self.options.indent(2);

        self.bump_sig()?; // `IN` or `OUT`
        let mut entries = Vec::new();

        loop {
            let (leading, gap_before) = self.take_leading_trivia();
            let had_blank = !entries.is_empty()
                && leading
                    .first()
                    .map_or(gap_before >= 2, |t| t.newlines_before >= 2);
            let mut inline = Vec::new();
            let pin_text = self.read_pin_decl(&mut inline)?;

            let mut pre_comma_comments = Vec::new();
            self.take_inline_trivia(&mut pre_comma_comments);
            inline.extend(pre_comma_comments);

            let has_comma = matches!(
                self.peek_sig().map(|t| &t.content),
                Some(TokenContent::Comma)
            );
            let trailing = if has_comma {
                self.bump_sig()?;
                self.take_same_line_comment()
            } else {
                None
            };

            entries.push(ListEntry {
                leading,
                text: pin_text,
                inline_comments: inline,
                trailing,
                had_blank_before: had_blank,
            });

            if !has_comma {
                break;
            }
        }

        let (before_semi, _) = self.take_leading_trivia();
        let semi = self.bump_sig()?;
        if semi.content != TokenContent::Semicolon {
            return None;
        }
        let trailing_after_semi = self.take_same_line_comment();

        let has_internal_comments = !before_semi.is_empty()
            || entries.iter().any(|e| {
                !e.leading.is_empty() || !e.inline_comments.is_empty() || e.trailing.is_some()
            });

        let single_line = {
            let joined: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
            format!("{keyword} {};", joined.join(", "))
        };
        let single_width = self.options.indent_width(1) + single_line.len();

        if !has_internal_comments && single_width <= self.options.max_line_width {
            self.out.push_str(&ind1);
            self.out.push_str(&single_line);
            if let Some(c) = trailing_after_semi {
                self.emit_trailing_comment(c, &ind1);
            }
            self.out.push('\n');
        } else {
            self.out.push_str(&ind1);
            self.out.push_str(keyword);
            self.out.push('\n');
            let last_idx = entries.len().saturating_sub(1);
            for (i, entry) in entries.iter().enumerate() {
                self.emit_trivia_block(&entry.leading, &ind2, i > 0, false);
                if entry.leading.is_empty() && entry.had_blank_before {
                    self.ensure_blank_line();
                }
                self.out.push_str(&ind2);
                self.out.push_str(&entry.text);
                for c in &entry.inline_comments {
                    self.out.push(' ');
                    self.out.push_str(&format_comment(c, &ind2));
                }
                if i < last_idx {
                    self.out.push(',');
                } else if before_semi.is_empty() {
                    self.out.push(';');
                }
                if let Some(c) = entry.trailing {
                    self.emit_trailing_comment(c, &ind2);
                } else if i == last_idx && before_semi.is_empty()
                    && let Some(c) = trailing_after_semi
                {
                    self.emit_trailing_comment(c, &ind2);
                }
                self.out.push('\n');
            }
            if !before_semi.is_empty() {
                self.emit_trivia_block(&before_semi, &ind2, true, false);
                self.out.push_str(&ind1);
                self.out.push(';');
                if let Some(c) = trailing_after_semi {
                    self.emit_trailing_comment(c, &ind1);
                }
                self.out.push('\n');
            }
        }
        Some(())
    }

    fn read_pin_decl(&mut self, inline: &mut Vec<&'a str>) -> Option<String> {
        let name_tok = self.bump_sig()?;
        let TokenContent::Ident(name) = &name_tok.content else {
            return None;
        };
        let mut s = name.clone();
        if matches!(
            self.peek_sig().map(|t| &t.content),
            Some(TokenContent::LeftBracket)
        ) {
            self.take_inline_trivia(inline);
            self.bump_sig()?; // `[`
            self.take_inline_trivia(inline);
            let width_tok = self.bump_sig()?;
            let TokenContent::Integer(w) = width_tok.content else {
                return None;
            };
            self.take_inline_trivia(inline);
            let rbrack = self.bump_sig()?;
            if rbrack.content != TokenContent::RightBracket {
                return None;
            }
            s.push('[');
            s.push_str(&w.to_string());
            s.push(']');
        }
        Some(s)
    }

    fn format_builtin_section(&mut self) -> Option<()> {
        let ind1 = self.options.indent(1);
        self.bump_sig()?; // `BUILTIN`
        let mut inline = Vec::new();
        self.take_inline_trivia(&mut inline);
        let name_tok = self.bump_sig()?;
        let TokenContent::Ident(name) = &name_tok.content else {
            return None;
        };
        self.take_inline_trivia(&mut inline);
        let semi = self.bump_sig()?;
        if semi.content != TokenContent::Semicolon {
            return None;
        }
        self.out.push_str(&ind1);
        self.out.push_str("BUILTIN ");
        self.out.push_str(name);
        for c in &inline {
            self.out.push(' ');
            self.out.push_str(&format_comment(c, &ind1));
        }
        self.out.push(';');
        if let Some(c) = self.take_same_line_comment() {
            self.emit_trailing_comment(c, &ind1);
        }
        self.out.push('\n');
        Some(())
    }

    fn format_clocked_section(&mut self) -> Option<()> {
        let ind1 = self.options.indent(1);
        self.bump_sig()?; // `CLOCKED`

        if matches!(
            self.peek_sig().map(|t| &t.content),
            Some(TokenContent::Semicolon)
        ) {
            let mut inline = Vec::new();
            self.take_inline_trivia(&mut inline);
            self.bump_sig()?; // `;`
            self.out.push_str(&ind1);
            self.out.push_str("CLOCKED");
            for c in &inline {
                self.out.push(' ');
                self.out.push_str(&format_comment(c, &ind1));
            }
            self.out.push(';');
            if let Some(c) = self.take_same_line_comment() {
                self.emit_trailing_comment(c, &ind1);
            }
            self.out.push('\n');
            return Some(());
        }

        let mut names = Vec::new();
        let mut inline = Vec::new();
        loop {
            self.take_inline_trivia(&mut inline);
            let tok = self.bump_sig()?;
            let TokenContent::Ident(n) = &tok.content else {
                return None;
            };
            names.push(n.as_str());
            self.take_inline_trivia(&mut inline);
            if matches!(
                self.peek_sig().map(|t| &t.content),
                Some(TokenContent::Comma)
            ) {
                self.bump_sig()?;
            } else {
                break;
            }
        }
        self.take_inline_trivia(&mut inline);
        let semi = self.bump_sig()?;
        if semi.content != TokenContent::Semicolon {
            return None;
        }
        self.out.push_str(&ind1);
        self.out.push_str("CLOCKED ");
        self.out.push_str(&names.join(", "));
        for c in &inline {
            self.out.push(' ');
            self.out.push_str(&format_comment(c, &ind1));
        }
        self.out.push(';');
        if let Some(c) = self.take_same_line_comment() {
            self.emit_trailing_comment(c, &ind1);
        }
        self.out.push('\n');
        Some(())
    }

    fn format_parts_section(&mut self) -> Option<()> {
        let ind1 = self.options.indent(1);
        self.bump_sig()?; // `PARTS`
        let mut inline = Vec::new();
        self.take_inline_trivia(&mut inline);
        let colon = self.bump_sig()?;
        if colon.content != TokenContent::Colon {
            return None;
        }

        self.out.push_str(&ind1);
        self.out.push_str("PARTS");
        for c in &inline {
            self.out.push(' ');
            self.out.push_str(&format_comment(c, &ind1));
        }
        self.out.push(':');
        if let Some(c) = self.take_same_line_comment() {
            self.emit_trailing_comment(c, &ind1);
        }
        self.out.push('\n');

        let mut part_count = 0usize;
        loop {
            // Peek ahead without consuming trivia if next significant token is `}`.
            if matches!(
                self.peek_sig().map(|t| &t.content),
                Some(TokenContent::RightBrace) | None
            ) {
                break;
            }
            let (leading, gap_before_part) = self.take_leading_trivia();
            if leading.is_empty() {
                if part_count > 0 && gap_before_part >= 2 {
                    self.ensure_blank_line();
                }
            } else {
                self.emit_trivia_block(&leading, &ind1, part_count > 0, false);
                if gap_before_part >= 2 {
                    self.ensure_blank_line();
                }
            }
            self.format_part()?;
            part_count += 1;
        }
        Some(())
    }

    fn format_part(&mut self) -> Option<()> {
        let ind1 = self.options.indent(1);
        let ind2 = self.options.indent(2);

        let name_tok = self.bump_sig()?;
        let TokenContent::Ident(chip_name) = &name_tok.content else {
            return None;
        };
        let mut pre_paren = Vec::new();
        self.take_inline_trivia(&mut pre_paren);
        let lparen = self.bump_sig()?;
        if lparen.content != TokenContent::LeftParen {
            return None;
        }
        let after_lparen_comment = self.take_same_line_comment();

        let mut entries = Vec::new();
        if !matches!(
            self.peek_sig().map(|t| &t.content),
            Some(TokenContent::RightParen)
        ) {
            loop {
                let (leading, gap_before) = self.take_leading_trivia();
                let had_blank = !entries.is_empty()
                    && leading
                        .first()
                        .map_or(gap_before >= 2, |t| t.newlines_before >= 2);
                let mut inline = Vec::new();
                let conn_text = self.read_connection(&mut inline)?;
                self.take_inline_trivia(&mut inline);

                let has_comma = matches!(
                    self.peek_sig().map(|t| &t.content),
                    Some(TokenContent::Comma)
                );
                let trailing = if has_comma {
                    self.bump_sig()?;
                    self.take_same_line_comment()
                } else {
                    self.take_same_line_comment()
                };

                entries.push(ListEntry {
                    leading,
                    text: conn_text,
                    inline_comments: inline,
                    trailing,
                    had_blank_before: had_blank,
                });

                if !has_comma {
                    break;
                }
            }
        }

        let (before_rparen, _) = self.take_leading_trivia();
        let rparen = self.bump_sig()?;
        if rparen.content != TokenContent::RightParen {
            return None;
        }
        let mut before_semi = Vec::new();
        self.take_inline_trivia(&mut before_semi);
        let semi = self.bump_sig()?;
        if semi.content != TokenContent::Semicolon {
            return None;
        }
        let trailing_after_semi = self.take_same_line_comment();

        let has_internal_comments = after_lparen_comment.is_some()
            || !before_rparen.is_empty()
            || entries.iter().any(|e| {
                !e.leading.is_empty() || !e.inline_comments.is_empty() || e.trailing.is_some()
            });

        let joined: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
        let single_line = format!("{chip_name}({});", joined.join(", "));
        let single_width = self.options.indent_width(1) + single_line.len();

        let multiline = !entries.is_empty()
            && (has_internal_comments || single_width > self.options.max_line_width);

        if !multiline && pre_paren.is_empty() && before_semi.is_empty() {
            self.out.push_str(&ind1);
            self.out.push_str(&single_line);
            if let Some(c) = trailing_after_semi {
                self.emit_trailing_comment(c, &ind1);
            }
            self.out.push('\n');
        } else if !multiline {
            self.out.push_str(&ind1);
            self.out.push_str(chip_name);
            for c in &pre_paren {
                self.out.push(' ');
                self.out.push_str(&format_comment(c, &ind1));
            }
            self.out.push('(');
            self.out.push_str(&joined.join(", "));
            self.out.push(')');
            for c in &before_semi {
                self.out.push(' ');
                self.out.push_str(&format_comment(c, &ind1));
            }
            self.out.push(';');
            if let Some(c) = trailing_after_semi {
                self.emit_trailing_comment(c, &ind1);
            }
            self.out.push('\n');
        } else {
            self.out.push_str(&ind1);
            self.out.push_str(chip_name);
            for c in &pre_paren {
                self.out.push(' ');
                self.out.push_str(&format_comment(c, &ind1));
            }
            self.out.push('(');
            if let Some(c) = after_lparen_comment {
                self.emit_trailing_comment(c, &ind1);
            }
            self.out.push('\n');

            let last_idx = entries.len().saturating_sub(1);
            for (i, entry) in entries.iter().enumerate() {
                self.emit_trivia_block(&entry.leading, &ind2, i > 0, false);
                if entry.leading.is_empty() && entry.had_blank_before {
                    self.ensure_blank_line();
                }
                self.out.push_str(&ind2);
                self.out.push_str(&entry.text);
                for c in &entry.inline_comments {
                    self.out.push(' ');
                    self.out.push_str(&format_comment(c, &ind2));
                }
                if i < last_idx {
                    self.out.push(',');
                }
                if let Some(c) = entry.trailing {
                    self.emit_trailing_comment(c, &ind2);
                }
                self.out.push('\n');
            }
            self.emit_trivia_block(&before_rparen, &ind2, true, false);
            self.out.push_str(&ind1);
            self.out.push(')');
            for c in &before_semi {
                self.out.push(' ');
                self.out.push_str(&format_comment(c, &ind1));
            }
            self.out.push(';');
            if let Some(c) = trailing_after_semi {
                self.emit_trailing_comment(c, &ind1);
            }
            self.out.push('\n');
        }

        Some(())
    }

    fn read_connection(&mut self, inline: &mut Vec<&'a str>) -> Option<String> {
        let lhs = self.read_pin_ref(inline)?;
        self.take_inline_trivia(inline);
        let eq = self.bump_sig()?;
        if eq.content != TokenContent::Equals {
            return None;
        }
        self.take_inline_trivia(inline);
        let rhs = match &self.peek()?.content {
            TokenContent::Keyword(Keyword::True) => {
                self.bump_sig()?;
                "true".to_string()
            }
            TokenContent::Keyword(Keyword::False) => {
                self.bump_sig()?;
                "false".to_string()
            }
            TokenContent::Ident(_) => self.read_pin_ref(inline)?,
            _ => return None,
        };
        Some(format!("{lhs}={rhs}"))
    }

    fn read_pin_ref(&mut self, inline: &mut Vec<&'a str>) -> Option<String> {
        let name_tok = self.bump_sig()?;
        let TokenContent::Ident(name) = &name_tok.content else {
            return None;
        };
        let mut s = name.clone();
        if matches!(
            self.peek_sig().map(|t| &t.content),
            Some(TokenContent::LeftBracket)
        ) {
            self.take_inline_trivia(inline);
            self.bump_sig()?; // `[`
            self.take_inline_trivia(inline);
            let start_tok = self.bump_sig()?;
            let TokenContent::Integer(start) = start_tok.content else {
                return None;
            };
            s.push('[');
            s.push_str(&start.to_string());
            if matches!(
                self.peek_sig().map(|t| &t.content),
                Some(TokenContent::DotDot)
            ) {
                self.take_inline_trivia(inline);
                self.bump_sig()?; // `..`
                self.take_inline_trivia(inline);
                let end_tok = self.bump_sig()?;
                let TokenContent::Integer(end) = end_tok.content else {
                    return None;
                };
                s.push_str("..");
                s.push_str(&end.to_string());
            }
            self.take_inline_trivia(inline);
            let rbrack = self.bump_sig()?;
            if rbrack.content != TokenContent::RightBracket {
                return None;
            }
            s.push(']');
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_compact_messy_chip() {
        let input = "CHIP   And{IN a,b ;OUT   out;PARTS: }";
        let expected = "\
CHIP And {
    IN a, b;
    OUT out;

    PARTS: }
";
        let formatted = format(input).expect("valid HDL should format");
        assert_eq!(formatted, expected);
        assert_eq!(format(&formatted).as_deref(), Some(expected));
    }

    #[test]
    fn preserves_comments_and_blank_line_groups() {
        let input = "\
// Header comment

/**
 * Doc comment
 */
CHIP Foo {
  IN a[ 16 ], b[16]; // inputs
  OUT out[16];
  PARTS:
  // Group 1
  Not16(in=a,out=na); // invert a


  // Group 2
  And16(a=na,b=b,out=out);
}
";
        let expected = "\
// Header comment

/**
 * Doc comment
 */
CHIP Foo {
    IN a[16], b[16]; // inputs
    OUT out[16];

    PARTS:
    // Group 1
    Not16(in=a, out=na); // invert a

    // Group 2
    And16(a=na, b=b, out=out);
}
";
        let formatted = format(input).unwrap();
        assert_eq!(formatted, expected);
        assert_eq!(format(&formatted).as_deref(), Some(expected));
    }

    #[test]
    fn wraps_long_part_calls_cleanly() {
        let input = "\
CHIP CPU {
    IN inM[16], instruction[16], reset;
    OUT outM[16];

    PARTS: }
";
        let formatted = format(input).unwrap();
        assert!(formatted.contains("    ALU(\n        x=d,\n"));
        assert!(formatted.contains("        ng=negative\n    );\n"));
        assert_eq!(format(&formatted).as_deref(), Some(formatted.as_str()));
    }

    #[test]
    fn formats_builtin_and_clocked_chips() {
        let input = "CHIP RAM8{IN in[16],load,address[3];OUT out[16];BUILTIN RAM8;CLOCKED in,load;}";
        let expected = "\
CHIP RAM8 {
    IN in[16], load, address[3];
    OUT out[16];

    BUILTIN RAM8;
    CLOCKED in, load;
}
";
        assert_eq!(format(input).as_deref(), Some(expected));
    }

    #[test]
    fn respects_tabs_and_custom_indent() {
        let input = "CHIP Not { IN in; OUT out; PARTS: }";
        let opts = FormatOptions {
            tab_size: 2,
            use_tabs: true,
            ..Default::default()
        };
        let formatted = format_with_options(input, &opts).unwrap();
        assert!(formatted.contains("\tIN in;\n"));
        assert!(formatted.contains("\tNand(a=in, b=in, out=out);\n"));
    }

    #[test]
    fn refuses_to_format_syntax_errors() {
        assert!(format("CHIP Broken { IN a; OUT out; PARTS: Nand(a=a ").is_none());
    }

    #[test]
    fn formats_all_sample_project_hdl_files_idempotently() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/projects");
        let mut count = 0;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().map(Result::unwrap) {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "hdl") {
                    count += 1;
                    let src = std::fs::read_to_string(&path).unwrap();
                    let once = format(&src)
                        .unwrap_or_else(|| panic!("failed to format {}", path.display()));
                    let twice = format(&once)
                        .unwrap_or_else(|| panic!("failed second format {}", path.display()));
                    assert_eq!(once, twice, "not idempotent for {}", path.display());
                    let reparsed = crate::hdl::parse(&once);
                    assert!(
                        reparsed.diagnostics.is_empty(),
                        "formatted {} had diagnostics: {:?}",
                        path.display(),
                        reparsed.diagnostics
                    );
                }
            }
        }
        assert!(count >= 25, "expected >= 25 sample hdl files, got {count}");
    }
}
