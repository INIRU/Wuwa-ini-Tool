use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use super::ClientError;

/// Identifies a server-owned external destination.
///
/// The webview sends only this closed enum. It can never provide a URL, path, or executable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalLinkKind {
    SourceCode,
    Releases,
    ReportIssue,
    KuroGamesOfficial,
}

impl ExternalLinkKind {
    pub const fn url(self) -> &'static str {
        match self {
            Self::SourceCode => "https://github.com/INIRU/Wuwa-ini-Tool",
            Self::Releases => "https://github.com/INIRU/Wuwa-ini-Tool/releases",
            Self::ReportIssue => "https://github.com/INIRU/Wuwa-ini-Tool/issues/new/choose",
            Self::KuroGamesOfficial => "https://wutheringwaves.kurogames.com/en/main/",
        }
    }
}

#[tauri::command]
pub fn open_external_link(app: AppHandle, kind: ExternalLinkKind) -> Result<(), ClientError> {
    app.opener()
        .open_url(kind.url(), None::<&str>)
        .map_err(|_| ClientError::new("external_link_unavailable"))
}
