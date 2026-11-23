//! Conversions between core types and LSP types.

use lsp_types as lsp;
use lsp_types::Url;
use n2t_core::diagnostic::{Diagnostic, Severity};
use n2t_core::{LineIndex, Position, Span};

/// A text plus its line index, for position conversions.
pub struct Text<'a> {
    pub text: &'a str,
    pub lines: &'a LineIndex,
}

impl<'a> Text<'a> {
    pub fn new(text: &'a str, lines: &'a LineIndex) -> Self {
        Self { text, lines }
    }

    pub fn position(&self, offset: usize) -> lsp::Position {
        let p = self.lines.position_utf16(self.text, offset);
        lsp::Position::new(p.line, p.column)
    }

    pub fn range(&self, span: Span) -> lsp::Range {
        lsp::Range::new(self.position(span.start), self.position(span.end))
    }

    pub fn offset(&self, position: lsp::Position) -> usize {
        self.lines
            .offset_utf16(
                self.text,
                Position {
                    line: position.line,
                    column: position.character,
                },
            )
            .unwrap_or(self.text.len())
    }
}

/// Converts a range in some other source text (read from disk if needed).
pub fn range_in(text: &str, span: Span) -> lsp::Range {
    let lines = LineIndex::new(text);
    Text::new(text, &lines).range(span)
}

/// The scheme of the read-only documents that show a built-in chip's
/// interface (`n2t-builtin:/And.hdl`). The editor serves their content from
/// the `n2t/builtinSource` request.
pub const BUILTIN_SCHEME: &str = "n2t-builtin";

/// Characters a path segment may keep unescaped in a `file:` URL.
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// The `file:` URL of an engine-form path (see [`n2t_core::host`]):
/// `C:/Users/me/And.hdl` becomes `file:///C:/Users/me/And.hdl`,
/// `//server/share/And.hdl` becomes `file://server/share/And.hdl`.
pub fn path_to_url(path: &str) -> Option<Url> {
    let encode = |text: &str| percent_encoding::utf8_percent_encode(text, PATH_SEGMENT).to_string();
    let text = if let Some(unc) = path.strip_prefix("//") {
        let (host, rest) = unc.split_once('/').unwrap_or((unc, ""));
        format!("file://{host}/{}", encode(rest))
    } else if path.len() >= 2 && path.as_bytes()[1] == b':' {
        format!("file:///{}:{}", &path[..1], encode(&path[2..]))
    } else if path.starts_with('/') {
        format!("file://{}", encode(path))
    } else {
        return None;
    };
    Url::parse(&text).ok()
}

/// The engine-form path of a `file:` URL, as editors write them: VS Code
/// sends `file:///c%3A/Users/me/My%20Chips/And.hdl` on Windows.
pub fn url_to_path(url: &Url) -> Option<String> {
    if url.scheme() != "file" {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(url.path())
        .decode_utf8()
        .ok()?
        .into_owned();
    if let Some(host) = url.host_str()
        && !host.is_empty()
        && host != "localhost"
    {
        return Some(format!("//{host}{decoded}"));
    }
    let bytes = decoded.as_bytes();
    // `/c:/Users/...` is a Windows drive path with a leading slash.
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return Some(n2t_core::host::path::normalize(&decoded[1..]));
    }
    Some(n2t_core::host::path::normalize(&decoded))
}

fn severity(severity: Severity) -> lsp::DiagnosticSeverity {
    match severity {
        Severity::Error => lsp::DiagnosticSeverity::ERROR,
        Severity::Warning => lsp::DiagnosticSeverity::WARNING,
        Severity::Information => lsp::DiagnosticSeverity::INFORMATION,
        Severity::Hint => lsp::DiagnosticSeverity::HINT,
    }
}

/// Converts a core diagnostic for a document. Related locations in other
/// sources are resolved by `resolve_source`, which maps a source identity to
/// its URL and text.
pub fn diagnostic(
    doc_uri: &Url,
    text: &Text<'_>,
    d: &Diagnostic,
    resolve_source: &mut dyn FnMut(&str) -> Option<(Url, String)>,
) -> lsp::Diagnostic {
    let range = d
        .span
        .map(|s| text.range(s))
        .unwrap_or_else(|| lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(0, 0)));
    let mut message = d.message.clone();
    let mut related = Vec::new();
    for r in &d.related {
        let location = r.location.as_ref().and_then(|loc| match &loc.source {
            None => Some(lsp::Location::new(doc_uri.clone(), text.range(loc.span))),
            Some(id) => resolve_source(id)
                .map(|(uri, src)| lsp::Location::new(uri, range_in(&src, loc.span))),
        });
        match location {
            Some(location) => related.push(lsp::DiagnosticRelatedInformation {
                location,
                message: r.message.clone(),
            }),
            None => {
                message.push_str("\nnote: ");
                message.push_str(&r.message);
            }
        }
    }
    lsp::Diagnostic {
        range,
        severity: Some(severity(d.severity)),
        code: Some(lsp::NumberOrString::String(d.code.as_str().to_string())),
        source: Some("n2t".to_string()),
        message,
        related_information: (!related.is_empty()).then_some(related),
        tags: matches!(
            d.code,
            n2t_core::DiagnosticCode::UnusedSignal
                | n2t_core::DiagnosticCode::UnusedInput
                | n2t_core::DiagnosticCode::Other("jack-unreachable-code")
        )
        .then(|| vec![lsp::DiagnosticTag::UNNECESSARY]),
        ..Default::default()
    }
}

/// Strips comment decoration (`*` gutters) from a documentation comment body.
pub fn clean_doc(doc: &str) -> String {
    let lines: Vec<&str> = doc
        .lines()
        .map(|l| {
            let t = l.trim();
            let t = t.strip_prefix('*').unwrap_or(t);
            t.strip_prefix(' ').unwrap_or(t)
        })
        .collect();
    let start = lines
        .iter()
        .position(|l| !l.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(start, |e| e + 1);
    lines[start..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(url: &str, path: &str) {
        let parsed = Url::parse(url).unwrap();
        assert_eq!(url_to_path(&parsed).as_deref(), Some(path), "{url}");
        let back = path_to_url(path).unwrap();
        assert_eq!(url_to_path(&back).as_deref(), Some(path), "{back}");
    }

    #[test]
    fn converts_file_urls_on_every_platform() {
        // What VS Code sends on Windows: lower-case drive, encoded colon.
        round_trip(
            "file:///c%3A/Users/me/OneDrive%20-%20Uni/nand2tetris/projects/01/And.hdl",
            "c:/Users/me/OneDrive - Uni/nand2tetris/projects/01/And.hdl",
        );
        round_trip("file:///C:/course/And.hdl", "C:/course/And.hdl");
        round_trip("file:///home/me/n2t/01/And.hdl", "/home/me/n2t/01/And.hdl");
        round_trip(
            "file://server/share/n2t/And.hdl",
            "//server/share/n2t/And.hdl",
        );
        round_trip(
            "file:///home/me/caf%C3%A9%20%23%251/And.hdl",
            "/home/me/caf\u{e9} #%1/And.hdl",
        );
        assert_eq!(
            path_to_url("c:/My Chips/And.hdl").unwrap().as_str(),
            "file:///c:/My%20Chips/And.hdl"
        );
        assert_eq!(
            url_to_path(&Url::parse("untitled:Untitled-1").unwrap()),
            None
        );
        assert_eq!(path_to_url("relative/And.hdl"), None);
    }
}
