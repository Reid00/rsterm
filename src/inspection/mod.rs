pub mod history;
pub mod network;
pub mod system;

pub use history::{HistoryEntry, HistorySnapshot, HistorySource};
pub use network::{ConnectionRow, NetworkSnapshot};
pub use system::{InterfaceSample, SystemSampler, SystemSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionKind {
    System,
    Network,
    History(HistorySource),
}

#[derive(Clone, Debug)]
pub enum InspectionData {
    System(SystemSnapshot),
    Network(NetworkSnapshot),
    History(HistorySnapshot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InspectionError {
    #[error("Inspection is unavailable for this session")]
    Unavailable,
    #[error("This operating system is not supported")]
    Unsupported,
    #[error("The required inspection tool is unavailable")]
    ToolUnavailable,
    #[error("The history file does not exist")]
    NotFound,
    #[error("Permission was denied")]
    PermissionDenied,
    #[error("The history source is not a safe regular file")]
    UnsafeFile,
    #[error("The host returned an unsupported inspection format")]
    InvalidData,
    #[error("The inspection exceeded its output limit")]
    OutputLimit,
    #[error("The host rejected the inspection request")]
    Rejected,
    #[error("The inspection failed")]
    Failed,
    #[error("The inspection timed out; cleanup may still be in progress")]
    Timeout,
    #[error("The inspection was cancelled")]
    Cancelled,
    #[error("An inspection is already active")]
    Busy,
    #[error("The previous inspection is still being cleaned up")]
    Cleaning,
}

impl InspectionError {
    pub fn stops_polling(self) -> bool {
        matches!(
            self,
            Self::Unavailable
                | Self::Unsupported
                | Self::ToolUnavailable
                | Self::PermissionDenied
                | Self::UnsafeFile
                | Self::InvalidData
                | Self::Rejected
                | Self::Cleaning
        )
    }
}
