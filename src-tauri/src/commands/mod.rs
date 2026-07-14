mod config;
pub(crate) mod external;
mod runtime;

pub use config::{
    ConfigCommandError, DiffKind, DiffLine, EngineIniPicker, GameRunningProbe, IniCommandService,
    IniPreview, NativeEngineIniPicker, PreviewSemanticChange, SelectedEngineIni,
    MAX_ENGINE_INI_BYTES,
};
pub use external::{open_external_link, ExternalLinkKind};
pub use runtime::*;

use serde::Serialize;

/// A deliberately narrow error returned across the IPC boundary.
///
/// Internal errors may contain filesystem paths or operating-system details. The frontend only
/// receives a stable code so those details never become part of a rendered error message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ClientError {
    pub code: &'static str,
}

impl ClientError {
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

impl From<ConfigCommandError> for ClientError {
    fn from(error: ConfigCommandError) -> Self {
        Self::new(error.code())
    }
}
