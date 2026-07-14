use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use super::{DiscoveryError, GameInstallation, InstallationChannel};

pub(crate) fn validate_with_channel(
    path: &Path,
    channel: InstallationChannel,
) -> Result<GameInstallation, DiscoveryError> {
    let (lexical_root, _) = validate_lexical_suffix(path)?;
    ensure_no_aliases(path, &lexical_root)?;

    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DiscoveryError::MissingExecutable(path.to_path_buf()));
        }
        Err(error) => return Err(DiscoveryError::io("executable_metadata", path, error)),
    };
    if !metadata.is_file() {
        return Err(DiscoveryError::NotAFile(path.to_path_buf()));
    }

    let executable = path
        .canonicalize()
        .map_err(|error| DiscoveryError::io("canonicalize_executable", path, error))?;
    let canonical_root = lexical_root
        .canonicalize()
        .map_err(|error| DiscoveryError::io("canonicalize_game_root", &lexical_root, error))?;
    let (derived_root, _) = validate_lexical_suffix(&executable)?;
    if derived_root != canonical_root {
        return Err(DiscoveryError::UnsafePathAlias(path.to_path_buf()));
    }

    let engine_ini = canonical_root.join("Client/Saved/Config/WindowsNoEditor/Engine.ini");
    validate_derived_config_path(&canonical_root, &engine_ini)?;

    Ok(GameInstallation {
        channel,
        game_root: canonical_root,
        executable,
        engine_ini,
    })
}

pub fn derive_engine_ini(executable: &Path) -> Result<PathBuf, DiscoveryError> {
    validate_with_channel(executable, InstallationChannel::Manual)
        .map(|installation| installation.engine_ini)
}

fn validate_lexical_suffix(path: &Path) -> Result<(PathBuf, PathBuf), DiscoveryError> {
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(DiscoveryError::UnsafePathAlias(path.to_path_buf()));
    }
    let executable_name = path.file_name().and_then(|name| name.to_str());
    let win64 = path.parent();
    let binaries = win64.and_then(Path::parent);
    let client = binaries.and_then(Path::parent);
    let game_root = client.and_then(Path::parent);
    if !component_eq(executable_name, "Client-Win64-Shipping.exe")
        || !component_eq(
            win64
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            "Win64",
        )
        || !component_eq(
            binaries
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            "Binaries",
        )
        || !component_eq(
            client
                .and_then(Path::file_name)
                .and_then(|name| name.to_str()),
            "Client",
        )
    {
        return Err(DiscoveryError::InvalidExecutablePath(path.to_path_buf()));
    }
    Ok((
        game_root
            .ok_or_else(|| DiscoveryError::InvalidExecutablePath(path.to_path_buf()))?
            .to_path_buf(),
        client.expect("validated Client parent").to_path_buf(),
    ))
}

fn component_eq(actual: Option<&str>, expected: &str) -> bool {
    actual.is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
}

fn ensure_no_aliases(path: &Path, game_root: &Path) -> Result<(), DiscoveryError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if is_alias(&metadata) => {
                return Err(DiscoveryError::UnsafePathAlias(candidate.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(DiscoveryError::MissingExecutable(path.to_path_buf()));
            }
            Err(error) => {
                return Err(DiscoveryError::io("path_metadata", candidate, error));
            }
        }
        if candidate == game_root {
            break;
        }
        current = candidate.parent();
    }
    Ok(())
}

fn validate_derived_config_path(game_root: &Path, engine_ini: &Path) -> Result<(), DiscoveryError> {
    let mut current = game_root.to_path_buf();
    for component in ["Client", "Saved", "Config", "WindowsNoEditor", "Engine.ini"] {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_alias(&metadata) => {
                return Err(DiscoveryError::UnsafePathAlias(current));
            }
            Ok(metadata) if component == "Engine.ini" && !metadata.is_file() => {
                return Err(DiscoveryError::InvalidConfigPath(current));
            }
            Ok(metadata) if component != "Engine.ini" && !metadata.is_dir() => {
                return Err(DiscoveryError::InvalidConfigPath(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(DiscoveryError::io("config_metadata", &current, error)),
        }
    }
    if current == engine_ini || engine_ini.starts_with(game_root) {
        Ok(())
    } else {
        Err(DiscoveryError::InvalidConfigPath(engine_ini.to_path_buf()))
    }
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
