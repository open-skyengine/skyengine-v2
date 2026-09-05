use std::path::PathBuf;

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid MRP package: {0}")]
    Package(String),
    #[error("package resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("package entry not found: {0}")]
    EntryNotFound(String),
    #[error("ambiguous package entry: {0}")]
    AmbiguousEntry(String),
    #[error("unsupported MR input: {0}")]
    UnsupportedMr(String),
    #[error("invalid MR chunk at byte {offset:#x}: {message}")]
    MrLoad { offset: usize, message: String },
    #[error("MR fault: {0}")]
    MrFault(String),
    #[error("ARM fault: {0}")]
    ArmFault(String),
    #[error("ABI error: {0}")]
    Abi(String),
    #[error("platform error: {0}")]
    Platform(String),
    #[error("invalid command line: {0}")]
    Config(String),
}

impl From<skyengine_arm::Error> for Error {
    fn from(error: skyengine_arm::Error) -> Self {
        match error {
            skyengine_arm::Error::ArmFault(message) => Self::ArmFault(message),
        }
    }
}

impl Error {
    pub(crate) fn mr_load(offset: usize, message: impl Into<String>) -> Self {
        Self::MrLoad {
            offset,
            message: message.into(),
        }
    }
}
