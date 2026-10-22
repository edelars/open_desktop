use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatError {
    TooLarge {
        kind: &'static str,
        max_bytes: usize,
    },
    Json(String),
    UnsupportedSchema(u32),
    InvalidField {
        field: &'static str,
        reason: String,
    },
    DigestMismatch,
    CaseMismatch(&'static str),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { kind, max_bytes } => {
                write!(f, "{kind} exceeds the {max_bytes}-byte limit")
            }
            Self::Json(message) => write!(f, "invalid JSON: {message}"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported schema version {version}")
            }
            Self::InvalidField { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::DigestMismatch => write!(f, "program SHA-256 does not match its bytes"),
            Self::CaseMismatch(field) => write!(f, "result does not match fixture field {field}"),
        }
    }
}

impl std::error::Error for FormatError {}
