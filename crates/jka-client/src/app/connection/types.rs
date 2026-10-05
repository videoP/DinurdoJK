//! Connection types.
use crate::app::{File, PathBuf, Receiver};

#[derive(Clone)]
pub(in crate::app) struct MissingMapPrompt {
    pub(in crate::app) map_name: String,
    pub(in crate::app) reason: String,
}

#[derive(Clone)]
pub(in crate::app) struct ServerPasswordPrompt {
    pub(in crate::app) target: String,
    pub(in crate::app) message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum LiveJoinUiPhase {
    CheckingContent,
    Synchronizing,
}

#[derive(Clone)]
pub(in crate::app) struct LiveJoinUiState {
    pub(in crate::app) map_name: String,
    pub(in crate::app) phase: LiveJoinUiPhase,
    pub(in crate::app) detail: String,
}

pub(in crate::app) enum LiveDownloadTransfer {
    Idle,
    Http {
        rx: Receiver<crate::download::HttpEvent>,
    },
    Legacy {
        file: File,
        temp_path: PathBuf,
        expected_block: u16,
        received: u64,
        total: Option<u64>,
    },
}

pub(in crate::app) struct LiveDownloadState {
    pub(in crate::app) map_name: String,
    pub(in crate::app) files: Vec<crate::download::DownloadSpec>,
    pub(in crate::app) index: usize,
    pub(in crate::app) http_base: Option<String>,
    pub(in crate::app) server_allows_legacy: bool,
    pub(in crate::app) transfer: LiveDownloadTransfer,
    pub(in crate::app) received: u64,
    pub(in crate::app) total: Option<u64>,
    pub(in crate::app) status: String,
}

impl LiveDownloadState {
    pub(in crate::app) fn current(&self) -> Option<&crate::download::DownloadSpec> {
        self.files.get(self.index)
    }
}
