mod error;
mod keyvalues;
mod paths;
#[cfg(target_os = "windows")]
mod windows_registry;

use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub use error::DiscoveryError;
pub use paths::derive_engine_ini;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationChannel {
    Kuro,
    Steam,
    Manual,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GameInstallation {
    pub channel: InstallationChannel,
    /// Discovery hints must not authorize configuration writes or game launch on their own.
    pub requires_user_confirmation: bool,
    pub game_root: PathBuf,
    pub executable: PathBuf,
    /// This exact path may not exist yet; creating it requires a separate user confirmation.
    pub engine_ini: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallEntry {
    pub display_name: String,
    pub publisher: Option<String>,
    pub install_location: PathBuf,
}

pub trait CandidateProvider {
    fn steam_roots(&self) -> Result<Vec<PathBuf>, DiscoveryError>;
    fn uninstall_entries(&self) -> Result<Vec<UninstallEntry>, DiscoveryError>;
}

pub fn validate_game_executable(
    path: impl AsRef<Path>,
) -> Result<GameInstallation, DiscoveryError> {
    paths::validate_with_channel(path.as_ref(), InstallationChannel::Manual)
}

pub fn discover_with_provider(
    provider: &impl CandidateProvider,
) -> Result<Vec<GameInstallation>, DiscoveryError> {
    let mut installations = Vec::new();
    for steam_root in provider.steam_roots()?.into_iter().take(16) {
        discover_steam_root(&steam_root, &mut installations);
    }
    for entry in provider.uninstall_entries()?.into_iter().take(16_384) {
        discover_kuro_entry(&entry, &mut installations);
    }
    deduplicate(&mut installations);
    installations.sort_by(|left, right| left.executable.cmp(&right.executable));
    Ok(installations)
}

#[cfg(target_os = "windows")]
pub fn discover_installations() -> Result<Vec<GameInstallation>, DiscoveryError> {
    discover_with_provider(&windows_registry::SystemCandidateProvider)
}

#[cfg(not(target_os = "windows"))]
pub fn discover_installations() -> Result<Vec<GameInstallation>, DiscoveryError> {
    Ok(Vec::new())
}

pub fn parse_library_folders(bytes: &[u8]) -> Result<Vec<PathBuf>, DiscoveryError> {
    keyvalues::parse_library_folders(bytes)
}

pub fn parse_app_manifest(bytes: &[u8]) -> Result<String, DiscoveryError> {
    keyvalues::parse_app_manifest(bytes)
}

fn discover_steam_root(steam_root: &Path, installations: &mut Vec<GameInstallation>) {
    if !is_safe_absolute_hint(steam_root) {
        return;
    }
    let library_file = steam_root.join("steamapps/libraryfolders.vdf");
    let mut libraries = vec![steam_root.to_path_buf()];
    if let Ok(bytes) = read_bounded_file(
        &library_file,
        steam_root,
        keyvalues::MAX_LIBRARY_FOLDERS_BYTES,
        "read_library_folders",
    ) {
        if let Ok(parsed) = parse_library_folders(&bytes) {
            libraries.extend(parsed);
        }
    }
    let mut seen_libraries = HashSet::new();
    for library in libraries {
        if !is_safe_absolute_hint(&library) {
            continue;
        }
        let key = path_key(&library);
        if !seen_libraries.insert(key) {
            continue;
        }
        let manifest = library.join("steamapps/appmanifest_3513350.acf");
        let Ok(bytes) = read_bounded_file(
            &manifest,
            &library,
            keyvalues::MAX_APP_MANIFEST_BYTES,
            "read_app_manifest",
        ) else {
            continue;
        };
        let Ok(install_dir) = parse_app_manifest(&bytes) else {
            continue;
        };
        if let Some(installation) = validate_steam_installation(&library, &install_dir) {
            installations.push(installation);
        }
    }
}

fn discover_kuro_entry(entry: &UninstallEntry, installations: &mut Vec<GameInstallation>) {
    if !matches_kuro_entry(entry) || !is_safe_absolute_hint(&entry.install_location) {
        return;
    }
    for game_root in [
        entry.install_location.clone(),
        entry.install_location.join("Wuthering Waves Game"),
    ] {
        let executable = game_root.join("Client/Binaries/Win64/Client-Win64-Shipping.exe");
        if let Ok(mut installation) =
            paths::validate_with_channel(&executable, InstallationChannel::Kuro)
        {
            installation.requires_user_confirmation = true;
            installations.push(installation);
        }
    }
}

fn matches_kuro_entry(entry: &UninstallEntry) -> bool {
    if entry.display_name.len() > 256
        || entry
            .publisher
            .as_ref()
            .is_some_and(|publisher| publisher.len() > 256)
    {
        return false;
    }
    let display_name = entry.display_name.to_lowercase();
    let game_name = display_name.contains("wuthering waves") || display_name.contains("명조");
    let publisher_matches = entry
        .publisher
        .as_ref()
        .is_none_or(|publisher| publisher.to_ascii_lowercase().contains("kuro"));
    game_name && publisher_matches
}

fn is_safe_absolute_hint(path: &Path) -> bool {
    path.as_os_str().len() <= 32_767
        && path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

fn read_bounded_file(
    path: &Path,
    boundary: &Path,
    maximum: usize,
    operation: &'static str,
) -> Result<Vec<u8>, DiscoveryError> {
    ensure_bounded_no_alias(path, boundary)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|error| DiscoveryError::io("metadata", path, error))?;
    if is_alias(&metadata) || !metadata.is_file() {
        return Err(DiscoveryError::InvalidKeyValues("metadata_file_type"));
    }
    let actual = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if actual > maximum {
        return Err(DiscoveryError::InputTooLarge { actual, maximum });
    }
    let file = fs::File::open(path).map_err(|error| DiscoveryError::io(operation, path, error))?;
    let mut bytes = Vec::with_capacity(actual);
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| DiscoveryError::io(operation, path, error))?;
    if bytes.len() > maximum {
        return Err(DiscoveryError::InputTooLarge {
            actual: bytes.len(),
            maximum,
        });
    }
    Ok(bytes)
}

fn validate_steam_installation(library: &Path, install_dir: &str) -> Option<GameInstallation> {
    let common = library.join("steamapps/common");
    let canonical_common = ensure_bounded_no_alias(&common, library).ok()?;
    let game_root = common.join(install_dir);
    let canonical_game_root = ensure_bounded_no_alias(&game_root, &common).ok()?;
    if canonical_game_root.parent() != Some(canonical_common.as_path()) {
        return None;
    }

    let executable = game_root.join("Client/Binaries/Win64/Client-Win64-Shipping.exe");
    let installation =
        paths::validate_with_channel(&executable, InstallationChannel::Steam).ok()?;
    (installation.game_root == canonical_game_root).then_some(installation)
}

fn ensure_bounded_no_alias(path: &Path, boundary: &Path) -> Result<PathBuf, DiscoveryError> {
    if !path.starts_with(boundary) {
        return Err(DiscoveryError::UnsafePathAlias(path.to_path_buf()));
    }
    let mut current = path;
    for _ in 0..16 {
        let metadata = fs::symlink_metadata(current)
            .map_err(|error| DiscoveryError::io("bounded_path_metadata", current, error))?;
        if is_alias(&metadata) {
            return Err(DiscoveryError::UnsafePathAlias(current.to_path_buf()));
        }
        if current == boundary {
            let canonical_boundary = boundary.canonicalize().map_err(|error| {
                DiscoveryError::io("canonicalize_bounded_root", boundary, error)
            })?;
            let canonical_path = path
                .canonicalize()
                .map_err(|error| DiscoveryError::io("canonicalize_bounded_path", path, error))?;
            if canonical_path == canonical_boundary
                || canonical_path.starts_with(&canonical_boundary)
            {
                return Ok(canonical_path);
            }
            return Err(DiscoveryError::UnsafePathAlias(path.to_path_buf()));
        }
        current = current
            .parent()
            .ok_or_else(|| DiscoveryError::UnsafePathAlias(path.to_path_buf()))?;
    }
    Err(DiscoveryError::UnsafePathAlias(path.to_path_buf()))
}

#[cfg(not(target_os = "windows"))]
fn is_alias(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(target_os = "windows")]
fn is_alias(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn deduplicate(installations: &mut Vec<GameInstallation>) {
    let mut seen = HashSet::new();
    installations.retain(|installation| seen.insert(path_key(&installation.executable)));
}

fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().into_owned();
    if cfg!(target_os = "windows") {
        value.to_ascii_lowercase()
    } else {
        value
    }
}
