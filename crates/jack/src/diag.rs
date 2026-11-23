//! Diagnostic construction shared by the parser, the checker and the
//! program-level pass.

use n2t_core::{Diagnostic, DiagnosticCode, Severity, Span};

/// A diagnostic plus the line the official compiler reports for the same
/// problem (0 when the official compiler says nothing about it).
#[derive(Debug, Clone)]
pub(crate) struct JDiag {
    pub diag: Diagnostic,
    pub java_line: u32,
    /// Sort key reproducing the official compiler's reporting order within a file.
    pub order: usize,
}

pub(crate) fn error(code: &'static str, msg: impl Into<String>, span: Span, java_line: u32) -> JDiag {
    JDiag {
        diag: Diagnostic::error(DiagnosticCode::Other(code), msg, span),
        java_line,
        order: span.start,
    }
}

pub(crate) fn warning(
    code: &'static str,
    msg: impl Into<String>,
    span: Span,
    java_line: u32,
) -> JDiag {
    JDiag {
        diag: Diagnostic::warning(DiagnosticCode::Other(code), msg, span),
        java_line,
        order: span.start,
    }
}

pub(crate) fn info(code: &'static str, msg: impl Into<String>, span: Span) -> JDiag {
    JDiag {
        diag: Diagnostic::new(
            Severity::Information,
            DiagnosticCode::Other(code),
            msg,
            Some(span),
        ),
        java_line: 0,
        order: span.start,
    }
}

impl JDiag {
    pub fn at_order(mut self, order: usize) -> Self {
        self.order = order;
        self
    }
    pub fn related(mut self, msg: impl Into<String>, span: Span) -> Self {
        self.diag = self.diag.with_related_span(msg, span);
        self
    }
}

/// Stable machine-readable codes used by this crate.
pub mod codes {
    pub const EXPECTED: &str = "jack-expected";
    pub const UNKNOWN_CHARACTER: &str = "jack-unknown-character";
    pub const BYTE_ORDER_MARK: &str = "jack-byte-order-mark";
    pub const UNCLOSED_STRING: &str = "jack-unclosed-string";
    pub const UNCLOSED_COMMENT: &str = "jack-unclosed-comment";
    pub const STRING_ESCAPE: &str = "jack-string-escape";
    pub const FRACTION: &str = "jack-fractional-number";
    pub const TOO_DEEP: &str = "jack-nesting-too-deep";
    pub const INTEGER_TOO_BIG: &str = "jack-integer-too-big";
    pub const UNDEFINED_VARIABLE: &str = "jack-undefined-variable";
    pub const TYPE_MISMATCH: &str = "jack-type-mismatch";
    pub const ILLEGAL_OPERAND: &str = "jack-illegal-operand";
    pub const STRING_CAST: &str = "jack-string-cast";
    pub const THIS_IN_FUNCTION: &str = "jack-this-in-function";
    pub const VOID_RETURN_VALUE: &str = "jack-void-return-value";
    pub const MISSING_RETURN_VALUE: &str = "jack-missing-return-value";
    pub const CONSTRUCTOR_RETURN: &str = "jack-constructor-return";
    pub const CONSTRUCTOR_TYPE: &str = "jack-constructor-type";
    pub const MISSING_RETURN: &str = "jack-missing-return";
    pub const UNREACHABLE: &str = "jack-unreachable-code";
    pub const METHOD_FROM_FUNCTION: &str = "jack-method-from-function";
    pub const DUPLICATE_SUBROUTINE: &str = "jack-duplicate-subroutine";
    pub const CLASS_NAME_MISMATCH: &str = "jack-class-name-mismatch";
    pub const DUPLICATE_VARIABLE: &str = "jack-duplicate-variable";
    pub const UNKNOWN_SUBROUTINE: &str = "jack-unknown-subroutine";
    pub const CALL_KIND: &str = "jack-call-kind";
    pub const ARGUMENT_COUNT: &str = "jack-argument-count";
    pub const DUPLICATE_CLASS: &str = "jack-duplicate-class";
}
