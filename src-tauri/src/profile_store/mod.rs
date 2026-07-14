mod error;
mod model;

use std::{
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

use crate::catalog::Catalog;

pub use error::ProfileError;
pub use model::{
    CpuSelection, CustomProfile, PriorityClass, ProcessProfile, ProfilePatch,
    PROFILE_SCHEMA_VERSION,
};

#[derive(Clone, Debug)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: app_data_dir.into().join("profiles"),
        }
    }

    pub fn list(&self) -> Result<Vec<CustomProfile>, ProfileError> {
        let directory = self.custom_directory();
        if !directory.exists() {
            return Ok(Vec::new());
        }
        ensure_existing_contained(&self.root, &directory)?;
        let mut profiles = fs::read_dir(&directory)
            .map_err(|error| ProfileError::io("read_directory", &directory, error))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .map(|entry| {
                let path = ensure_existing_contained(&self.root, &entry.path())?;
                read_profile(&path)
            })
            .collect::<Result<Vec<_>, ProfileError>>()?;
        profiles.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(profiles)
    }

    pub fn get(&self, id: &str) -> Result<CustomProfile, ProfileError> {
        validate_identifier(id)?;
        let path = self.profile_path(id);
        if !path.is_file() {
            return Err(ProfileError::ProfileNotFound(id.to_owned()));
        }
        read_profile(&ensure_existing_contained(&self.root, &path)?)
    }

    pub fn save(
        &self,
        profile: &CustomProfile,
        catalog: &Catalog,
    ) -> Result<CustomProfile, ProfileError> {
        validate_profile(profile, catalog)?;
        atomic_write_json(&self.root, &self.profile_path(&profile.id), profile)?;
        Ok(profile.clone())
    }

    pub fn rename(&self, id: &str, new_name: &str) -> Result<CustomProfile, ProfileError> {
        validate_display_name(new_name)?;
        let mut profile = self.get(id)?;
        profile.name = new_name.trim().to_owned();
        atomic_write_json(&self.root, &self.profile_path(id), &profile)?;
        Ok(profile)
    }

    pub fn clone_profile(
        &self,
        id: &str,
        new_id: &str,
        new_name: &str,
    ) -> Result<CustomProfile, ProfileError> {
        validate_identifier(new_id)?;
        validate_display_name(new_name)?;
        let destination = self.profile_path(new_id);
        if destination.exists() {
            return Err(ProfileError::ProfileAlreadyExists(new_id.to_owned()));
        }
        let mut profile = self.get(id)?;
        profile.id = new_id.to_owned();
        profile.name = new_name.trim().to_owned();
        atomic_write_json(&self.root, &destination, &profile)?;
        Ok(profile)
    }

    pub fn export(&self, id: &str, file_name: &str) -> Result<PathBuf, ProfileError> {
        validate_export_file_name(file_name)?;
        let profile = self.get(id)?;
        let destination = self.root.join("exports").join(file_name);
        ensure_lexically_contained(&self.root, &destination)?;
        atomic_write_json(&self.root, &destination, &profile)?;
        Ok(destination)
    }

    pub fn import(&self, source: &Path, catalog: &Catalog) -> Result<CustomProfile, ProfileError> {
        let canonical_source = ensure_existing_contained(&self.root, source)?;
        let profile = read_profile(&canonical_source)?;
        if self.profile_path(&profile.id).exists() {
            return Err(ProfileError::ProfileAlreadyExists(profile.id));
        }
        self.save(&profile, catalog)
    }

    fn custom_directory(&self) -> PathBuf {
        self.root.join("custom")
    }

    fn profile_path(&self, id: &str) -> PathBuf {
        self.custom_directory().join(format!("{id}.json"))
    }
}

fn validate_profile(profile: &CustomProfile, catalog: &Catalog) -> Result<(), ProfileError> {
    if profile.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedSchemaVersion(
            profile.schema_version,
        ));
    }
    if profile.patch.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedSchemaVersion(
            profile.patch.schema_version,
        ));
    }
    validate_identifier(&profile.id)?;
    validate_display_name(&profile.name)?;
    validate_process(&profile.patch.process)?;

    for change in &profile.patch.managed_ini {
        let option = catalog
            .options
            .get(&change.key)
            .ok_or_else(|| ProfileError::UnknownProfileKey(change.key.clone()))?;
        if option.section != change.section {
            return Err(ProfileError::InvalidProfile("catalog_section_mismatch"));
        }
        if let Some(value) = change.value.as_deref() {
            crate::catalog::validate_option(option, value)
                .map_err(|_| ProfileError::InvalidProfile("invalid_option_value"))?;
        }
    }
    Ok(())
}

fn validate_process(process: &ProcessProfile) -> Result<(), ProfileError> {
    match &process.cpu_selection {
        CpuSelection::ManualCpuSets { ids } if ids.is_empty() => {
            Err(ProfileError::InvalidProfile("empty_cpu_set_selection"))
        }
        CpuSelection::ManualCpuSets { ids } => {
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.len() == ids.len() {
                Ok(())
            } else {
                Err(ProfileError::InvalidProfile("duplicate_cpu_set"))
            }
        }
        CpuSelection::HardAffinity { mask: 0, .. } => {
            Err(ProfileError::InvalidProfile("empty_affinity_mask"))
        }
        _ => Ok(()),
    }
}

fn validate_identifier(id: &str) -> Result<(), ProfileError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ProfileError::InvalidName(id.to_owned()));
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<(), ProfileError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 80 || trimmed.chars().any(char::is_control) {
        return Err(ProfileError::InvalidName(name.to_owned()));
    }
    Ok(())
}

fn validate_export_file_name(file_name: &str) -> Result<(), ProfileError> {
    let path = Path::new(file_name);
    let is_single_component = matches!(
        path.components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    );
    let safe_characters = file_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if !is_single_component
        || !safe_characters
        || !file_name.ends_with(".json")
        || file_name.len() > 80
    {
        return Err(ProfileError::InvalidFileName(file_name.to_owned()));
    }
    Ok(())
}

fn ensure_lexically_contained(root: &Path, candidate: &Path) -> Result<(), ProfileError> {
    if candidate.starts_with(root)
        && !candidate
            .strip_prefix(root)
            .expect("candidate starts with root")
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        Ok(())
    } else {
        Err(ProfileError::PathOutsideStore(candidate.to_path_buf()))
    }
}

fn read_profile(path: &Path) -> Result<CustomProfile, ProfileError> {
    let bytes = fs::read(path).map_err(|error| ProfileError::io("read", path, error))?;
    let profile: CustomProfile = serde_json::from_slice(&bytes)?;
    if profile.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedSchemaVersion(
            profile.schema_version,
        ));
    }
    if profile.patch.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedSchemaVersion(
            profile.patch.schema_version,
        ));
    }
    Ok(profile)
}

fn atomic_write_json<T: serde::Serialize>(
    root: &Path,
    path: &Path,
    value: &T,
) -> Result<(), ProfileError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProfileError::PathOutsideStore(path.to_path_buf()))?;
    fs::create_dir_all(parent)
        .map_err(|error| ProfileError::io("create_directory", parent, error))?;
    let canonical_root = canonical_store_root(root)?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| ProfileError::io("canonicalize_directory", parent, error))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(ProfileError::PathOutsideStore(canonical_parent));
    }
    if path.exists() {
        ensure_existing_contained(root, path)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| ProfileError::io("create_temporary", parent, error))?;
    temporary
        .write_all(&bytes)
        .map_err(|error| ProfileError::io("write_temporary", temporary.path(), error))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| ProfileError::io("sync_temporary", temporary.path(), error))?;
    temporary
        .persist(path)
        .map_err(|error| ProfileError::io("persist", path, error.error))?;
    sync_parent(parent)?;
    Ok(())
}

fn canonical_store_root(root: &Path) -> Result<PathBuf, ProfileError> {
    let app_data = root
        .parent()
        .ok_or_else(|| ProfileError::PathOutsideStore(root.to_path_buf()))?;
    fs::create_dir_all(app_data)
        .map_err(|error| ProfileError::io("create_app_data", app_data, error))?;
    fs::create_dir_all(root).map_err(|error| ProfileError::io("create_store", root, error))?;
    let canonical_app_data = fs::canonicalize(app_data)
        .map_err(|error| ProfileError::io("canonicalize_app_data", app_data, error))?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| ProfileError::io("canonicalize_store", root, error))?;
    if canonical_root.parent() != Some(canonical_app_data.as_path()) {
        return Err(ProfileError::PathOutsideStore(canonical_root));
    }
    Ok(canonical_root)
}

fn ensure_existing_contained(root: &Path, path: &Path) -> Result<PathBuf, ProfileError> {
    let canonical_root = canonical_store_root(root)?;
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| ProfileError::io("canonicalize_path", path, error))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(ProfileError::PathOutsideStore(canonical_path));
    }
    Ok(canonical_path)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<(), ProfileError> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ProfileError::io("sync_parent", parent, error))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<(), ProfileError> {
    Ok(())
}
