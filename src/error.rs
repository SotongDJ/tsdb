use std::fmt;
use std::io;

#[derive(Debug)]
pub enum TsdbError {
    Io(io::Error),
    EscapeError(String),
    InvalidUuid(String),
    ParseError { line: usize, message: String },
    DuplicateUuid(String),
    MissingUuid(String),
    LockConflict { pid: u64, uuids: Vec<String> },
    Other(String),
}

impl fmt::Display for TsdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TsdbError::Io(e) => write!(f, "I/O error: {}", e),
            TsdbError::EscapeError(s) => write!(f, "Escape error: {}", s),
            TsdbError::InvalidUuid(s) => write!(f, "Invalid UUID: {}", s),
            TsdbError::ParseError { line, message } => {
                write!(f, "Parse error at line {}: {}", line, message)
            }
            TsdbError::DuplicateUuid(u) => write!(f, "Duplicate UUID: {}", u),
            TsdbError::MissingUuid(u) => write!(f, "Missing UUID: {}", u),
            TsdbError::LockConflict { pid, uuids } => write!(
                f,
                "Lock conflict with PID {}: overlapping UUIDs: {}",
                pid,
                uuids.join(", ")
            ),
            TsdbError::Other(s) => write!(f, "{}", s),
        }
    }
}

impl From<io::Error> for TsdbError {
    fn from(e: io::Error) -> Self {
        TsdbError::Io(e)
    }
}

impl std::error::Error for TsdbError {}

pub type Result<T> = std::result::Result<T, TsdbError>;
