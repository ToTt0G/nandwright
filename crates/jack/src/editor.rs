//! Diagnostics as an editor should show them.
//!
//! The compiler's diagnostics say what the official JackCompiler would
//! reject. Two situations in the course's own files are errors to the
//! compiler and yet nothing a student did wrong, so an editor that painted
//! them red would be crying wolf:
//!
//! - **A subroutine that is not written yet.** Project 12 hands out the OS
//!   classes as skeletons whose subroutine bodies are empty. The compiler
//!   rejects each one for lacking a `return`. In an editor that is a hint.
//! - **The project 10 `ExpressionLessSquare` folder.** Its code is
//!   deliberately meaningless (every expression is replaced by a variable
//!   name) so that a parser without expression handling can be tested on it.
//!   It is syntactically valid and is not meant to compile.

use n2t_core::diagnostic::{Diagnostic, Severity};

use crate::ParsedJack;
use crate::diag::codes;

/// The folder whose code only has to parse.
const PARSER_EXERCISE_FOLDER: &str = "ExpressionLessSquare";

/// Codes of problems in the text itself, as opposed to its meaning.
const SYNTAX_CODES: &[&str] = &[
    codes::EXPECTED,
    codes::UNKNOWN_CHARACTER,
    codes::BYTE_ORDER_MARK,
    codes::UNCLOSED_STRING,
    codes::UNCLOSED_COMMENT,
    codes::TOO_DEEP,
    codes::INTEGER_TOO_BIG,
];

/// Adapts compiler diagnostics for display. `folder_name` is the name of the
/// folder the file is in (not its path).
pub fn editor_diagnostics(
    parsed: &ParsedJack,
    folder_name: &str,
    diagnostics: &[Diagnostic],
) -> Vec<Diagnostic> {
    let unwritten: Vec<_> = parsed
        .class
        .iter()
        .flat_map(|class| &class.subroutines)
        .filter(|sub| sub.statements.is_empty() && sub.locals.is_empty())
        .collect();
    let parser_exercise = folder_name == PARSER_EXERCISE_FOLDER;
    diagnostics
        .iter()
        .map(|d| {
            let mut d = d.clone();
            if !d.is_error() {
                return d;
            }
            let code = d.code.as_str();
            let stub = d.span.and_then(|span| {
                unwritten
                    .iter()
                    .find(|sub| sub.span.start <= span.start && span.end <= sub.span.end)
            });
            if let Some(sub) = stub
                && (code == codes::MISSING_RETURN || code == codes::CONSTRUCTOR_RETURN)
            {
                let name = sub.name.as_ref().map_or("this subroutine", |n| n.name.as_str());
                d.severity = Severity::Hint;
                d.message = format!(
                    "`{name}` is not written yet. When it is, it must end with a `return` statement."
                );
            } else if parser_exercise && !SYNTAX_CODES.contains(&code) {
                d.severity = Severity::Hint;
                d.message.push_str(
                    " (The code in this folder is deliberately meaningless: it is there to test a parser, not to be compiled.)",
                );
            }
            d
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(folder: &str, source: &str) -> Vec<(Severity, String)> {
        let compiled = crate::compile("Thing", source);
        editor_diagnostics(&crate::parse(source), folder, &compiled.diagnostics)
            .into_iter()
            .map(|d| (d.severity, d.code.as_str().to_string()))
            .collect()
    }

    #[test]
    fn an_unwritten_subroutine_is_a_hint() {
        let source = "class Thing {\n    function int size() {\n    }\n    constructor Thing new() {\n    }\n}\n";
        let diagnostics = shown("12", source);
        assert!(!diagnostics.is_empty());
        assert!(
            diagnostics
                .iter()
                .all(|(severity, _)| *severity == Severity::Hint),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn a_written_subroutine_without_return_is_still_an_error() {
        let source =
            "class Thing {\n    function void run() {\n        do Output.println();\n    }\n}\n";
        let diagnostics = shown("09", source);
        assert_eq!(
            diagnostics,
            [(Severity::Error, codes::MISSING_RETURN.to_string())]
        );
    }

    #[test]
    fn the_parser_exercise_folder_is_not_painted_red() {
        // A boolean where an int is declared: an error anywhere else.
        let source = "class Thing {\n    function void run() {\n        var int x;\n        let x = true;\n        return;\n    }\n}\n";
        assert!(
            shown("09", source)
                .iter()
                .any(|(s, _)| *s == Severity::Error)
        );
        let softened = shown("ExpressionLessSquare", source);
        assert!(
            softened.iter().all(|(s, _)| *s == Severity::Hint),
            "{softened:?}"
        );
        // A syntax error there is still an error.
        let broken = shown(
            "ExpressionLessSquare",
            "class Thing {\n    function void run( {\n",
        );
        assert!(
            broken.iter().any(|(s, _)| *s == Severity::Error),
            "{broken:?}"
        );
    }
}
