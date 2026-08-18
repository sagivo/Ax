//! Diagnostics. Permanent append-only codes. Versioned JSON schema.
//!
//! `safety` ∈ { semantics_preserving, interface_widening, behavior_changing }.
//! Only `semantics_preserving` fixes are ever auto-applied.

use crate::span::Span;
use serde::{Deserialize, Serialize};

pub const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixSafety {
    SemanticsPreserving,
    InterfaceWidening,
    BehaviorChanging,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fix {
    pub kind: String,
    pub safety: FixSafety,
    pub rank: u32,
    pub patch: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub schema: u32,
    pub code: String,
    pub severity: Severity,
    pub def_id: Option<String>,
    pub node_path: Vec<serde_json::Value>,
    pub kind: String,
    pub msg: String,
    pub msg_key: String,
    pub span: SpanJson,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub expected_row: Option<Vec<String>>,
    pub actual_row: Option<Vec<String>>,
    pub injection: Option<String>,
    pub fixes: Vec<Fix>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpanJson {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl From<Span> for SpanJson {
    fn from(s: Span) -> Self {
        Self {
            file: s.file.0,
            start: s.start,
            end: s.end,
        }
    }
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, span: Span, msg: impl Into<String>) -> Self {
        let code = code.into();
        let msg = msg.into();
        Self {
            schema: DIAGNOSTIC_SCHEMA_VERSION,
            kind: kind_from_code(&code),
            msg_key: code.clone(),
            code,
            severity: Severity::Error,
            def_id: None,
            node_path: Vec::new(),
            msg,
            span: span.into(),
            expected: None,
            actual: None,
            expected_row: None,
            actual_row: None,
            injection: None,
            fixes: Vec::new(),
        }
    }

    pub fn warn(code: impl Into<String>, span: Span, msg: impl Into<String>) -> Self {
        let mut d = Self::error(code, span, msg);
        d.severity = Severity::Warning;
        d
    }

    pub fn with_rows(mut self, expected: Vec<String>, actual: Vec<String>) -> Self {
        self.expected_row = Some(expected);
        self.actual_row = Some(actual);
        self
    }

    pub fn with_expected_actual(mut self, expected: String, actual: String) -> Self {
        self.expected = Some(expected);
        self.actual = Some(actual);
        self
    }

    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fixes.push(fix);
        self
    }

    pub fn with_def(mut self, id: impl Into<String>) -> Self {
        self.def_id = Some(id.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

fn kind_from_code(code: &str) -> String {
    match code {
        "E0001" => "lex_error",
        "E0002" | "E0004" | "E0005" | "E0006" | "E0007" | "E0008" => "parse_error",
        "E0003" => "unknown_effect",
        "E0100" => "unknown_name",
        "E0101" => "type_mismatch",
        "E0102" => "unknown_type",
        "E0103" => "arity_mismatch",
        "E0104" => "unknown_field",
        "E0105" => "unknown_variant",
        "E0106" => "not_a_function",
        "E0107" => "not_indexable",
        "E0108" => "implicit_conversion",
        "E0200" => "effect_not_permitted",
        "E0201" => "duplicate_err_in_row",
        "E0202" => "missing_injection",
        "E0203" => "ambiguous_injection",
        "E0204" => "catch_not_exhaustive",
        "E0300" => "region_store_illegal",
        "E0301" => "exclusive_borrow",
        "E0302" => "escaping_borrow",
        "E0303" => "no_reborrow",
        "E0400" => "dict_ambiguous",
        "E0401" => "dict_missing",
        "E0402" => "unknown_dict",
        "E0500" => "hole_not_allowed",
        "E0501" => "contract_illegal",
        "E0502" => "strict_det_violation",
        "E0600" => "par_not_disjoint",
        "E0700" => "trusted_ffi_strict",
        _ => "generic",
    }
    .into()
}

/// Permanent, append-only error-code catalog. Used by `ax card` and tests.
pub fn catalog() -> Vec<(&'static str, &'static str)> {
    vec![
        ("E0001", "lexer error"),
        ("E0002", "expected declaration"),
        ("E0003", "unknown effect"),
        ("E0004", "expected test name"),
        ("E0005", "expected expression"),
        ("E0006", "expected pattern"),
        ("E0007", "expected identifier"),
        ("E0008", "unexpected token"),
        ("E0100", "unknown name"),
        ("E0101", "type mismatch"),
        ("E0102", "unknown type"),
        ("E0103", "arity mismatch"),
        ("E0104", "unknown field"),
        ("E0105", "unknown variant"),
        ("E0106", "not a function"),
        ("E0107", "not indexable"),
        ("E0108", "implicit numeric conversion forbidden"),
        ("E0200", "effect not permitted by declared row"),
        ("E0201", "at most one err[E] per concrete row"),
        ("E0202", "missing declared injection"),
        ("E0203", "ambiguous injection"),
        ("E0204", "catch not exhaustive"),
        ("E0300", "illegal region store (r must outlive location)"),
        ("E0301", "exclusive mutable borrow violated"),
        ("E0302", "borrow escapes its region"),
        ("E0303", "reborrowing is not permitted in v1"),
        ("E0400", "ambiguous default dictionary"),
        ("E0401", "no visible default dictionary"),
        ("E0402", "unknown dictionary"),
        ("E0500", "typed hole rejected (not --allow-holes)"),
        ("E0501", "illegal construct in contract sublanguage"),
        ("E0502", "--strict-det rejects race/nondet/io"),
        ("E0600", "par mutable captures are not statically disjoint"),
        ("E0700", "raw native FFI forbidden in strict mode"),
    ]
}
