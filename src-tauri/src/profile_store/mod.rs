mod error;
mod model;

use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::catalog::Catalog;
use crate::ini_document::ManagedChange;

pub use error::ProfileError;
pub use model::{
    CpuSelection, CustomEntryProvenance, CustomIniEntry, CustomProfile, ImportPreview,
    ImportWarning, PortableProfile, PriorityClass, ProcessProfile, ProfileExport, ProfilePatch,
    ProfileShareEnvelope, ShareProvenance, ShareWarning, MAX_SHARE_BYTES, PROFILE_SCHEMA_VERSION,
    SHARE_SCHEMA_VERSION,
};

const MAX_MANAGED_ENTRIES: usize = 256;
const MAX_CUSTOM_ENTRIES: usize = 256;
const MAX_CPU_IDS: usize = 256;
const MAX_SECTION_CHARS: usize = 128;
const MAX_KEY_CHARS: usize = 256;
const MAX_VALUE_CHARS: usize = 8192;
const MAX_NAME_CHARS: usize = 80;
const MAX_APP_VERSION_CHARS: usize = 32;
static PROFILE_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

impl ProfilePatch {
    /// Validates every profile field before exposing changes to the INI merge pipeline.
    pub fn validated_managed_changes(
        &self,
        catalog: &Catalog,
    ) -> Result<Vec<ManagedChange>, ProfileError> {
        validate_patch(self, catalog)?;
        let mut changes = self
            .managed_ini
            .iter()
            .map(|change| match &change.value {
                Some(value) => ManagedChange::set(&change.section, &change.key, value),
                None => ManagedChange::delete(&change.section, &change.key),
            })
            .collect::<Vec<_>>();
        changes.extend(
            self.custom_ini_entries
                .iter()
                .map(|entry| ManagedChange::set(&entry.section, &entry.key, &entry.value)),
        );
        Ok(changes)
    }
}

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
        if !directory
            .try_exists()
            .map_err(|error| ProfileError::io("check_directory", &directory, error))?
        {
            return Ok(Vec::new());
        }
        ensure_existing_contained(&self.root, &directory)?;
        let catalog =
            Catalog::load_embedded().map_err(|_| ProfileError::InvalidProfile("catalog"))?;
        let mut profiles = Vec::new();
        let mut normalized_ids = BTreeSet::new();
        for entry in fs::read_dir(&directory)
            .map_err(|error| ProfileError::io("read_directory", &directory, error))?
        {
            let entry = entry
                .map_err(|error| ProfileError::io("read_directory_entry", &directory, error))?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let expected_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or(ProfileError::InvalidProfile("profile_filename"))?;
            let profile = read_validated_profile(&self.root, &path, expected_id, &catalog)?;
            if !normalized_ids.insert(profile.id.to_ascii_lowercase()) {
                return Err(ProfileError::InvalidProfile(
                    "case_insensitive_id_collision",
                ));
            }
            profiles.push(profile);
        }
        profiles.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(profiles)
    }

    pub fn get(&self, id: &str) -> Result<CustomProfile, ProfileError> {
        validate_identifier(id)?;
        let path = self.profile_path(id);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProfileError::ProfileNotFound(id.to_owned()));
            }
            Err(error) => return Err(ProfileError::io("profile_metadata", &path, error)),
        }
        let catalog =
            Catalog::load_embedded().map_err(|_| ProfileError::InvalidProfile("catalog"))?;
        read_validated_profile(&self.root, &path, id, &catalog)
    }

    pub fn save(
        &self,
        profile: &CustomProfile,
        catalog: &Catalog,
    ) -> Result<CustomProfile, ProfileError> {
        let mut candidate = profile.clone();
        candidate.name = candidate.name.trim().to_owned();
        validate_profile(&candidate, catalog)?;
        let path = self.profile_path(&candidate.id);
        with_profile_lock(&path, || {
            if path
                .try_exists()
                .map_err(|error| ProfileError::io("check_profile", &path, error))?
            {
                if candidate.revision == 0 {
                    return Err(ProfileError::ProfileAlreadyExists(candidate.id.clone()));
                }
                let stored = read_validated_profile(&self.root, &path, &candidate.id, catalog)?;
                if candidate.revision != stored.revision {
                    return Err(ProfileError::RevisionConflict {
                        expected: candidate.revision,
                        actual: stored.revision,
                    });
                }
                candidate.revision = stored
                    .revision
                    .checked_add(1)
                    .ok_or(ProfileError::InvalidProfile("revision_overflow"))?;
                atomic_write_json(
                    &self.root,
                    &path,
                    &candidate,
                    WriteMode::Replace,
                    &candidate.id,
                )?;
            } else {
                ensure_no_case_insensitive_collision(&self.custom_directory(), &candidate.id)?;
                if candidate.revision != 0 {
                    return Err(ProfileError::RevisionConflict {
                        expected: candidate.revision,
                        actual: 0,
                    });
                }
                candidate.revision = 1;
                atomic_write_json(
                    &self.root,
                    &path,
                    &candidate,
                    WriteMode::Create,
                    &candidate.id,
                )?;
            }
            Ok(candidate)
        })
    }

    pub fn rename(&self, id: &str, new_name: &str) -> Result<CustomProfile, ProfileError> {
        validate_identifier(id)?;
        validate_display_name(new_name)?;
        let path = self.profile_path(id);
        with_profile_lock(&path, || {
            let catalog =
                Catalog::load_embedded().map_err(|_| ProfileError::InvalidProfile("catalog"))?;
            let mut profile = read_validated_profile(&self.root, &path, id, &catalog)?;
            profile.name = new_name.trim().to_owned();
            profile.revision = profile
                .revision
                .checked_add(1)
                .ok_or(ProfileError::InvalidProfile("revision_overflow"))?;
            validate_profile(&profile, &catalog)?;
            atomic_write_json(&self.root, &path, &profile, WriteMode::Replace, id)?;
            Ok(profile)
        })
    }

    pub fn clone_profile(
        &self,
        id: &str,
        new_id: &str,
        new_name: &str,
    ) -> Result<CustomProfile, ProfileError> {
        validate_identifier(new_id)?;
        validate_display_name(new_name)?;
        let source = self.get(id)?;
        let destination = self.profile_path(new_id);
        with_profile_lock(&destination, || {
            if destination
                .try_exists()
                .map_err(|error| ProfileError::io("check_profile", &destination, error))?
            {
                return Err(ProfileError::ProfileAlreadyExists(new_id.to_owned()));
            }
            ensure_no_case_insensitive_collision(&self.custom_directory(), new_id)?;
            let mut profile = source.clone();
            profile.id = new_id.to_owned();
            profile.name = new_name.trim().to_owned();
            profile.revision = 1;
            atomic_write_json(
                &self.root,
                &destination,
                &profile,
                WriteMode::Create,
                new_id,
            )?;
            Ok(profile)
        })
    }

    pub fn export(&self, id: &str) -> Result<ProfileExport, ProfileError> {
        let profile = self.get(id)?;
        let mut patch = profile.patch;
        let mut portability_warnings = Vec::new();
        if matches!(
            patch.process.cpu_selection,
            CpuSelection::ManualCpuSets { .. } | CpuSelection::HardAffinity { .. }
        ) {
            patch.process.cpu_selection = CpuSelection::All;
            portability_warnings.push(ShareWarning::DeviceSpecificCpuExcluded);
        }
        let envelope = ProfileShareEnvelope {
            schema_version: SHARE_SCHEMA_VERSION,
            creating_app_version: env!("CARGO_PKG_VERSION").to_owned(),
            provenance: ShareProvenance::WuwaIniToolProfile,
            exported_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|_| ProfileError::InvalidProfile("export_timestamp"))?,
            portability_warnings,
            profile: PortableProfile {
                name: profile.name,
                patch,
            },
        };
        let bytes = serde_json::to_vec_pretty(&envelope)?;
        enforce_share_size(bytes.len() as u64)?;
        Ok(ProfileExport {
            suggested_file_name: format!("{}.wuwaprofile.json", profile.id),
            bytes,
        })
    }

    pub fn import(&self, source: &Path, catalog: &Catalog) -> Result<ImportPreview, ProfileError> {
        let metadata = fs::metadata(source)
            .map_err(|error| ProfileError::io("share_metadata", source, error))?;
        if !metadata.is_file() {
            return Err(ProfileError::InvalidProfile("share_must_be_regular_file"));
        }
        enforce_share_size(metadata.len())?;
        let file =
            File::open(source).map_err(|error| ProfileError::io("open_share", source, error))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_SHARE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| ProfileError::io("read_share", source, error))?;
        enforce_share_size(bytes.len() as u64)?;
        self.import_bytes(&bytes, catalog)
    }

    pub fn import_bytes(
        &self,
        bytes: &[u8],
        catalog: &Catalog,
    ) -> Result<ImportPreview, ProfileError> {
        enforce_share_size(bytes.len() as u64)?;
        let envelope: ProfileShareEnvelope = serde_json::from_slice(bytes)?;
        if envelope.schema_version != SHARE_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchemaVersion(
                envelope.schema_version,
            ));
        }
        if envelope.creating_app_version.is_empty()
            || envelope.creating_app_version.chars().count() > MAX_APP_VERSION_CHARS
            || envelope.creating_app_version.chars().any(char::is_control)
        {
            return Err(ProfileError::InvalidProfile("creating_app_version"));
        }
        let unique_share_warnings = envelope
            .portability_warnings
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if envelope.portability_warnings.len() > 16
            || unique_share_warnings.len() != envelope.portability_warnings.len()
        {
            return Err(ProfileError::InvalidProfile("portability_warnings"));
        }
        OffsetDateTime::parse(&envelope.exported_at, &Rfc3339)
            .map_err(|_| ProfileError::InvalidProfile("exported_at"))?;
        validate_display_name(&envelope.profile.name)?;
        validate_patch(&envelope.profile.patch, catalog)?;

        let mut patch = envelope.profile.patch;
        let mut warnings = Vec::new();
        if matches!(
            patch.process.cpu_selection,
            CpuSelection::ManualCpuSets { .. } | CpuSelection::HardAffinity { .. }
        ) {
            patch.process.cpu_selection = CpuSelection::All;
            warnings.push(ImportWarning::DeviceSpecificCpuReset);
        }
        if envelope
            .portability_warnings
            .contains(&ShareWarning::DeviceSpecificCpuExcluded)
            && !warnings.contains(&ImportWarning::DeviceSpecificCpuReset)
        {
            warnings.push(ImportWarning::DeviceSpecificCpuReset);
        }
        if matches!(
            patch.process.priority,
            PriorityClass::High | PriorityClass::Realtime
        ) {
            warnings.push(ImportWarning::ElevatedPriority);
        }
        Ok(ImportPreview {
            display_name: envelope.profile.name,
            patch,
            warnings,
            source_app_version: envelope.creating_app_version,
            exported_at: envelope.exported_at,
        })
    }

    pub fn save_import(
        &self,
        preview: &ImportPreview,
        id: &str,
        name: &str,
        catalog: &Catalog,
    ) -> Result<CustomProfile, ProfileError> {
        let candidate = CustomProfile {
            schema_version: PROFILE_SCHEMA_VERSION,
            id: id.to_owned(),
            name: name.trim().to_owned(),
            revision: 0,
            patch: preview.patch.clone(),
        };
        validate_profile(&candidate, catalog)?;
        let path = self.profile_path(id);
        let creation_lock = self.root.join(".creation-lock");
        with_profile_lock(&creation_lock, || {
            if path
                .try_exists()
                .map_err(|error| ProfileError::io("check_profile", &path, error))?
            {
                return Err(ProfileError::ProfileAlreadyExists(id.to_owned()));
            }
            ensure_no_case_insensitive_collision(&self.custom_directory(), id)?;
            ensure_no_display_name_collision(self, name)?;
            let mut stored = candidate;
            stored.revision = 1;
            atomic_write_json(&self.root, &path, &stored, WriteMode::Create, id)?;
            Ok(stored)
        })
    }

    fn custom_directory(&self) -> PathBuf {
        self.root.join("custom")
    }

    fn profile_path(&self, id: &str) -> PathBuf {
        self.custom_directory().join(format!("{id}.json"))
    }
}

fn ensure_no_display_name_collision(store: &ProfileStore, name: &str) -> Result<(), ProfileError> {
    let normalized = name.trim().to_lowercase();
    if store
        .list()?
        .iter()
        .any(|profile| profile.name.to_lowercase() == normalized)
    {
        Err(ProfileError::ProfileNameAlreadyExists(
            name.trim().to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn validate_profile(profile: &CustomProfile, catalog: &Catalog) -> Result<(), ProfileError> {
    if profile.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedSchemaVersion(
            profile.schema_version,
        ));
    }
    validate_identifier(&profile.id)?;
    validate_display_name(&profile.name)?;
    validate_patch(&profile.patch, catalog)
}

fn validate_patch(patch: &ProfilePatch, catalog: &Catalog) -> Result<(), ProfileError> {
    if patch.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedSchemaVersion(patch.schema_version));
    }
    if patch.managed_ini.len() > MAX_MANAGED_ENTRIES {
        return Err(ProfileError::InvalidProfile("too_many_managed_entries"));
    }
    if patch.custom_ini_entries.len() > MAX_CUSTOM_ENTRIES {
        return Err(ProfileError::InvalidProfile("too_many_custom_entries"));
    }
    validate_process(&patch.process)?;
    let mut identities = BTreeSet::new();
    for change in &patch.managed_ini {
        let option = catalog
            .options
            .get(&change.key)
            .ok_or_else(|| ProfileError::UnknownProfileKey(change.key.clone()))?;
        if option.section != change.section {
            return Err(ProfileError::InvalidProfile("catalog_section_mismatch"));
        }
        insert_identity(&mut identities, &change.section, &change.key)?;
        if let Some(value) = change.value.as_deref() {
            crate::catalog::validate_option(option, value)
                .map_err(|_| ProfileError::InvalidProfile("invalid_option_value"))?;
        }
    }
    for entry in &patch.custom_ini_entries {
        validate_custom_entry(entry)?;
        insert_identity(&mut identities, &entry.section, &entry.key)?;
    }
    Ok(())
}

fn validate_custom_entry(entry: &CustomIniEntry) -> Result<(), ProfileError> {
    if entry.provenance != CustomEntryProvenance::Custom || entry.runtime_verified {
        return Err(ProfileError::InvalidProfile("custom_entry_provenance"));
    }
    validate_ini_piece(&entry.section, MAX_SECTION_CHARS, "custom_section")?;
    if trim_ascii(&entry.section) != entry.section {
        return Err(ProfileError::InvalidProfile("custom_section_whitespace"));
    }
    if entry.section.contains(['[', ']']) {
        return Err(ProfileError::InvalidProfile("custom_section_delimiter"));
    }
    validate_ini_piece(&entry.key, MAX_KEY_CHARS, "custom_key")?;
    if trim_ascii(&entry.key) != entry.key {
        return Err(ProfileError::InvalidProfile("custom_key_whitespace"));
    }
    if entry.key.starts_with([';', '#']) {
        return Err(ProfileError::InvalidProfile("custom_key_comment"));
    }
    if entry.key.contains('=') {
        return Err(ProfileError::InvalidProfile("custom_key_delimiter"));
    }
    validate_ini_piece(&entry.value, MAX_VALUE_CHARS, "custom_value")?;
    if trim_ascii(&entry.value) != entry.value {
        return Err(ProfileError::InvalidProfile("custom_value_whitespace"));
    }
    if entry.value.starts_with([';', '#']) {
        return Err(ProfileError::InvalidProfile("custom_value_comment"));
    }
    if entry
        .value
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0].is_ascii_whitespace() && matches!(pair[1], b';' | b'#'))
    {
        return Err(ProfileError::InvalidProfile("custom_value_inline_comment"));
    }
    Ok(())
}

fn trim_ascii(value: &str) -> &str {
    value.trim_matches(|character: char| character.is_ascii_whitespace())
}

fn validate_ini_piece(
    value: &str,
    maximum_chars: usize,
    reason: &'static str,
) -> Result<(), ProfileError> {
    if value.is_empty()
        || value.chars().count() > maximum_chars
        || value.chars().any(char::is_control)
    {
        return Err(ProfileError::InvalidProfile(reason));
    }
    Ok(())
}

fn insert_identity(
    identities: &mut BTreeSet<(String, String)>,
    section: &str,
    key: &str,
) -> Result<(), ProfileError> {
    if !identities.insert((section.to_ascii_lowercase(), key.to_ascii_lowercase())) {
        return Err(ProfileError::InvalidProfile("duplicate_ini_identity"));
    }
    Ok(())
}

fn validate_process(process: &ProcessProfile) -> Result<(), ProfileError> {
    match &process.cpu_selection {
        CpuSelection::ManualCpuSets { ids } if ids.is_empty() => {
            Err(ProfileError::InvalidProfile("empty_cpu_set_selection"))
        }
        CpuSelection::ManualCpuSets { ids } if ids.len() > MAX_CPU_IDS => {
            Err(ProfileError::InvalidProfile("too_many_cpu_sets"))
        }
        CpuSelection::ManualCpuSets { ids } => {
            let unique = ids.iter().copied().collect::<BTreeSet<_>>();
            if unique.len() == ids.len() {
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
    let valid_characters = id.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
    });
    if id.is_empty() || id.len() > 64 || !valid_characters || is_windows_reserved(id) {
        return Err(ProfileError::InvalidName(id.to_owned()));
    }
    Ok(())
}

fn is_windows_reserved(value: &str) -> bool {
    let normalized = value.trim_end_matches(['.', ' ']);
    let stem = normalized
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn validate_display_name(name: &str) -> Result<(), ProfileError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_NAME_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(ProfileError::InvalidName(name.to_owned()));
    }
    Ok(())
}

fn read_validated_profile(
    root: &Path,
    path: &Path,
    expected_id: &str,
    catalog: &Catalog,
) -> Result<CustomProfile, ProfileError> {
    let path = ensure_existing_contained(root, path)?;
    let metadata =
        fs::metadata(&path).map_err(|error| ProfileError::io("profile_metadata", &path, error))?;
    if !metadata.is_file() {
        return Err(ProfileError::InvalidProfile("profile_must_be_regular_file"));
    }
    enforce_share_size(metadata.len())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&path)
        .map_err(|error| ProfileError::io("open_profile", &path, error))?
        .take(MAX_SHARE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ProfileError::io("read_profile", &path, error))?;
    enforce_share_size(bytes.len() as u64)?;
    let profile: CustomProfile = serde_json::from_slice(&bytes)?;
    if profile.id != expected_id {
        return Err(ProfileError::InvalidProfile("filename_payload_id_mismatch"));
    }
    if profile.revision == 0 {
        return Err(ProfileError::InvalidProfile(
            "stored_revision_must_be_positive",
        ));
    }
    validate_profile(&profile, catalog)?;
    Ok(profile)
}

fn enforce_share_size(actual: u64) -> Result<(), ProfileError> {
    if actual > MAX_SHARE_BYTES {
        Err(ProfileError::ShareTooLarge {
            actual,
            maximum: MAX_SHARE_BYTES,
        })
    } else {
        Ok(())
    }
}

fn ensure_no_case_insensitive_collision(directory: &Path, id: &str) -> Result<(), ProfileError> {
    if !directory
        .try_exists()
        .map_err(|error| ProfileError::io("check_directory", directory, error))?
    {
        return Ok(());
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| ProfileError::io("read_directory", directory, error))?
    {
        let entry =
            entry.map_err(|error| ProfileError::io("read_directory_entry", directory, error))?;
        if entry
            .path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case(id))
        {
            return Err(ProfileError::ProfileAlreadyExists(id.to_owned()));
        }
    }
    Ok(())
}

fn with_profile_lock<T>(
    path: &Path,
    operation: impl FnOnce() -> Result<T, ProfileError>,
) -> Result<T, ProfileError> {
    let registry = PROFILE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let lock = {
        let mut locks = registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        locks
            .entry(path.to_path_buf())
            .or_default()
            .upgrade()
            .unwrap_or_else(|| {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
                lock
            })
    };
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    operation()
}

#[derive(Clone, Copy)]
enum WriteMode {
    Create,
    Replace,
}

fn atomic_write_json<T: serde::Serialize>(
    root: &Path,
    path: &Path,
    value: &T,
    mode: WriteMode,
    id: &str,
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
    if matches!(mode, WriteMode::Replace) {
        ensure_existing_contained(root, path)?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    enforce_share_size(bytes.len() as u64)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| ProfileError::io("create_temporary", parent, error))?;
    temporary
        .write_all(&bytes)
        .map_err(|error| ProfileError::io("write_temporary", temporary.path(), error))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| ProfileError::io("sync_temporary", temporary.path(), error))?;
    match mode {
        WriteMode::Create => temporary.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                ProfileError::ProfileAlreadyExists(id.to_owned())
            } else {
                ProfileError::io("persist_create", path, error.error)
            }
        })?,
        WriteMode::Replace => temporary
            .persist(path)
            .map_err(|error| ProfileError::io("persist_replace", path, error.error))?,
    };
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
