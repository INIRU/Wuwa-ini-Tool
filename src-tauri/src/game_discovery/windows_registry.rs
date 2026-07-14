use std::path::PathBuf;

use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY},
    RegKey,
};

use super::{CandidateProvider, DiscoveryError, UninstallEntry};

const UNINSTALL_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const MAX_REGISTRY_SUBKEYS: usize = 4096;

pub(crate) struct SystemCandidateProvider;

impl CandidateProvider for SystemCandidateProvider {
    fn steam_roots(&self) -> Result<Vec<PathBuf>, DiscoveryError> {
        let mut roots = Vec::new();
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let hive = RegKey::predef(hive);
            for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
                let Ok(key) = hive.open_subkey_with_flags(r"Software\Valve\Steam", KEY_READ | view)
                else {
                    continue;
                };
                for value_name in ["SteamPath", "InstallPath"] {
                    if let Ok(value) = key.get_value::<String, _>(value_name) {
                        if value.len() <= 32_767 {
                            roots.push(PathBuf::from(value));
                        }
                    }
                }
            }
        }
        roots.sort();
        roots.dedup();
        Ok(roots)
    }

    fn uninstall_entries(&self) -> Result<Vec<UninstallEntry>, DiscoveryError> {
        let mut entries = Vec::new();
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let hive = RegKey::predef(hive);
            for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
                let Ok(uninstall) = hive.open_subkey_with_flags(UNINSTALL_PATH, KEY_READ | view)
                else {
                    continue;
                };
                for name in uninstall.enum_keys().take(MAX_REGISTRY_SUBKEYS).flatten() {
                    let Ok(product) = uninstall.open_subkey_with_flags(&name, KEY_READ | view)
                    else {
                        continue;
                    };
                    let Ok(display_name) = product.get_value::<String, _>("DisplayName") else {
                        continue;
                    };
                    let Ok(install_location) = product.get_value::<String, _>("InstallLocation")
                    else {
                        continue;
                    };
                    if display_name.len() > 256 || install_location.len() > 32_767 {
                        continue;
                    }
                    let publisher = product
                        .get_value::<String, _>("Publisher")
                        .ok()
                        .filter(|value| value.len() <= 256);
                    entries.push(UninstallEntry {
                        display_name,
                        publisher,
                        install_location: PathBuf::from(install_location),
                    });
                }
            }
        }
        Ok(entries)
    }
}
