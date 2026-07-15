use std::{fs, path::PathBuf};

use tempfile::TempDir;
use wuwa_ini_tool_lib::game_discovery::{
    discover_installations, discover_with_provider, parse_app_manifest, parse_library_folders,
    validate_game_executable, CandidateProvider, DiscoveryError, InstallationChannel,
    UninstallEntry,
};

const EXE_NAME: &str = "Client-Win64-Shipping.exe";

struct GameFixture {
    _temp: TempDir,
    game_root: PathBuf,
    executable: PathBuf,
}

impl GameFixture {
    fn new(root_name: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game_root = temp.path().join(root_name);
        let executable = game_root
            .join("Client")
            .join("Binaries")
            .join("Win64")
            .join(EXE_NAME);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"fixture executable").unwrap();
        Self {
            _temp: temp,
            game_root,
            executable,
        }
    }

    fn engine_ini(&self) -> PathBuf {
        self.game_root
            .canonicalize()
            .unwrap()
            .join("Client/Saved/Config/WindowsNoEditor/Engine.ini")
    }
}

#[derive(Default)]
struct StaticProvider {
    steam_roots: Vec<PathBuf>,
    uninstall_entries: Vec<UninstallEntry>,
}

impl CandidateProvider for StaticProvider {
    fn steam_roots(&self) -> Result<Vec<PathBuf>, DiscoveryError> {
        Ok(self.steam_roots.clone())
    }

    fn uninstall_entries(&self) -> Result<Vec<UninstallEntry>, DiscoveryError> {
        Ok(self.uninstall_entries.clone())
    }
}

#[test]
fn manual_selection_derives_only_the_validated_engine_ini_leaf() {
    let fixture = GameFixture::new("Wuthering Waves Game");

    let result = validate_game_executable(&fixture.executable).unwrap();

    assert_eq!(result.channel, InstallationChannel::Manual);
    assert!(!result.requires_user_confirmation);
    assert_eq!(result.game_root, fixture.game_root.canonicalize().unwrap());
    assert_eq!(
        result.executable,
        fixture.executable.canonicalize().unwrap()
    );
    assert_eq!(result.engine_ini, fixture.engine_ini());
}

#[test]
fn absent_engine_ini_is_a_derived_candidate_but_alternate_files_are_rejected() {
    let fixture = GameFixture::new("game");
    assert!(!fixture.engine_ini().exists());

    let result = validate_game_executable(&fixture.executable).unwrap();
    assert_eq!(result.engine_ini, fixture.engine_ini());

    let alternate = fixture
        .game_root
        .join("Client/Saved/Config/WindowsNoEditor/UserEngine.ini");
    fs::create_dir_all(alternate.parent().unwrap()).unwrap();
    fs::write(&alternate, b"[SystemSettings]").unwrap();
    assert!(matches!(
        validate_game_executable(&alternate),
        Err(DiscoveryError::InvalidExecutablePath(_))
    ));
}

#[test]
fn validates_windows_components_ascii_case_insensitively() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp
        .path()
        .join("game/cLiEnT/BINARIES/wIn64/CLIENT-win64-SHIPPING.EXE");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"exe").unwrap();

    let result = validate_game_executable(&executable).unwrap();

    assert_eq!(
        result.game_root,
        temp.path().join("game").canonicalize().unwrap()
    );
    assert!(result
        .engine_ini
        .ends_with("Client/Saved/Config/WindowsNoEditor/Engine.ini"));
}

#[test]
fn rejects_wrong_missing_and_non_file_executables() {
    let fixture = GameFixture::new("game");
    let wrong = fixture.executable.with_file_name("Wuthering Waves.exe");
    fs::write(&wrong, b"launcher").unwrap();
    let missing_root = tempfile::tempdir().unwrap();
    let missing = missing_root
        .path()
        .join("game/Client/Binaries/Win64")
        .join(EXE_NAME);
    fs::create_dir_all(missing.parent().unwrap()).unwrap();
    let directory_root = tempfile::tempdir().unwrap();
    let directory = directory_root
        .path()
        .join("game/Client/Binaries/Win64")
        .join(EXE_NAME);
    fs::create_dir_all(&directory).unwrap();

    assert!(matches!(
        validate_game_executable(wrong),
        Err(DiscoveryError::InvalidExecutablePath(_))
    ));
    assert!(matches!(
        validate_game_executable(missing),
        Err(DiscoveryError::MissingExecutable(_))
    ));
    assert!(matches!(
        validate_game_executable(directory),
        Err(DiscoveryError::NotAFile(_))
    ));
}

#[cfg(unix)]
#[test]
fn rejects_a_client_symlink_that_escapes_the_selected_game_root() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let selected_root = temp.path().join("selected");
    let outside_root = temp.path().join("outside");
    let outside_executable = outside_root.join("Client/Binaries/Win64").join(EXE_NAME);
    fs::create_dir_all(outside_executable.parent().unwrap()).unwrap();
    fs::write(&outside_executable, b"exe").unwrap();
    fs::create_dir_all(&selected_root).unwrap();
    symlink(outside_root.join("Client"), selected_root.join("Client")).unwrap();
    let selected_executable = selected_root.join("Client/Binaries/Win64").join(EXE_NAME);

    assert!(matches!(
        validate_game_executable(selected_executable),
        Err(DiscoveryError::UnsafePathAlias(_))
    ));
}

#[cfg(unix)]
#[test]
fn rejects_an_engine_ini_symlink_that_escapes_the_validated_tree() {
    use std::os::unix::fs::symlink;

    let fixture = GameFixture::new("game");
    let outside = fixture.game_root.parent().unwrap().join("outside.ini");
    fs::write(&outside, b"[SystemSettings]").unwrap();
    fs::create_dir_all(fixture.engine_ini().parent().unwrap()).unwrap();
    symlink(&outside, fixture.engine_ini()).unwrap();

    assert!(matches!(
        validate_game_executable(&fixture.executable),
        Err(DiscoveryError::UnsafePathAlias(_))
    ));
}

#[test]
fn discovers_user_reported_program_files_shape_from_kuro_hint() {
    let fixture = GameFixture::new("Program Files/Wuthering Waves/Wuthering Waves Game");
    let install_location = fixture.game_root.parent().unwrap().to_path_buf();
    let provider = StaticProvider {
        uninstall_entries: vec![UninstallEntry {
            display_name: "Wuthering Waves".into(),
            publisher: Some("KURO GAMES".into()),
            install_location,
        }],
        ..StaticProvider::default()
    };

    let installations = discover_with_provider(&provider).unwrap();

    assert_eq!(installations.len(), 1);
    assert_eq!(installations[0].channel, InstallationChannel::Kuro);
    assert!(installations[0].requires_user_confirmation);
    assert_eq!(installations[0].engine_ini, fixture.engine_ini());
}

#[test]
fn uninstall_hint_without_a_publisher_still_requires_user_confirmation() {
    let fixture = GameFixture::new("Wuthering Waves Game");
    let provider = StaticProvider {
        uninstall_entries: vec![UninstallEntry {
            display_name: "Wuthering Waves".into(),
            publisher: None,
            install_location: fixture.game_root.clone(),
        }],
        ..StaticProvider::default()
    };

    let installations = discover_with_provider(&provider).unwrap();

    assert_eq!(installations.len(), 1);
    assert!(installations[0].requires_user_confirmation);
}

#[test]
fn ignores_unrelated_uninstall_and_local_appdata_hints() {
    let temp = tempfile::tempdir().unwrap();
    let local_engine = temp
        .path()
        .join("LOCALAPPDATA/WutheringWaves/Saved/Config/WindowsNoEditor/Engine.ini");
    fs::create_dir_all(local_engine.parent().unwrap()).unwrap();
    fs::write(&local_engine, b"[SystemSettings]").unwrap();
    let provider = StaticProvider {
        uninstall_entries: vec![UninstallEntry {
            display_name: "Unrelated Launcher".into(),
            publisher: Some("Unknown".into()),
            install_location: temp.path().join("LOCALAPPDATA"),
        }],
        ..StaticProvider::default()
    };

    assert!(discover_with_provider(&provider).unwrap().is_empty());
}

#[test]
fn discovers_non_default_steam_library_from_bounded_metadata() {
    let steam = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    let steam_vdf_path = steam.path().to_string_lossy().replace('\\', "\\\\");
    let library_vdf_path = library.path().to_string_lossy().replace('\\', "\\\\");
    let game_root = library.path().join("steamapps/common/Wuthering Waves");
    let executable = game_root.join("Client/Binaries/Win64").join(EXE_NAME);
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"exe").unwrap();
    fs::create_dir_all(steam.path().join("steamapps")).unwrap();
    fs::write(
        steam.path().join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}",
            steam_vdf_path, library_vdf_path
        ),
    )
    .unwrap();
    fs::create_dir_all(library.path().join("steamapps")).unwrap();
    fs::write(
        library.path().join("steamapps/appmanifest_3513350.acf"),
        "\"AppState\" { \"appid\" \"3513350\" \"installdir\" \"Wuthering Waves\" }",
    )
    .unwrap();
    let provider = StaticProvider {
        steam_roots: vec![steam.path().to_path_buf()],
        ..StaticProvider::default()
    };

    let installations = discover_with_provider(&provider).unwrap();

    assert_eq!(installations.len(), 1);
    assert_eq!(installations[0].channel, InstallationChannel::Steam);
    assert!(!installations[0].requires_user_confirmation);
    assert_eq!(
        installations[0].game_root,
        game_root.canonicalize().unwrap()
    );
}

#[test]
fn rejects_malformed_duplicate_and_oversized_keyvalues() {
    assert!(matches!(
        parse_library_folders(b"\"libraryfolders\" { \"0\""),
        Err(DiscoveryError::InvalidKeyValues(_))
    ));
    assert!(matches!(
        parse_app_manifest(
            b"\"AppState\" { \"appid\" \"3513350\" \"appid\" \"3513350\" \"installdir\" \"game\" }"
        ),
        Err(DiscoveryError::InvalidKeyValues(_))
    ));
    let oversized = vec![b' '; 2 * 1024 * 1024 + 1];
    assert!(matches!(
        parse_library_folders(&oversized),
        Err(DiscoveryError::InputTooLarge { .. })
    ));
    let oversized_manifest = vec![b' '; 512 * 1024 + 1];
    assert!(matches!(
        parse_app_manifest(&oversized_manifest),
        Err(DiscoveryError::InputTooLarge { .. })
    ));
}

#[test]
fn rejects_traversal_or_path_shaped_steam_install_directory() {
    for installdir in [
        "../outside",
        "folder/child",
        r"C:\\outside",
        ".",
        "..",
        "game.",
        "game ",
        " game",
        "game\tname",
        "game<name",
        "game>name",
        "game|name",
        "game?name",
        "game*name",
        "game:name",
    ] {
        let manifest =
            format!("\"AppState\" {{ \"appid\" \"3513350\" \"installdir\" \"{installdir}\" }}");
        assert!(
            parse_app_manifest(manifest.as_bytes()).is_err(),
            "installdir must be one safe directory component: {installdir}"
        );
    }
}

#[test]
fn rejects_windows_reserved_steam_install_directory_basenames() {
    for installdir in [
        "CON",
        "con.txt",
        "CON .txt",
        "PRN",
        "AUX.cfg",
        "NUL",
        "CLOCK$",
        "COM1",
        "com9.log",
        "com1 .log",
        "LPT1",
        "lpt9.data",
    ] {
        let manifest =
            format!("\"AppState\" {{ \"appid\" \"3513350\" \"installdir\" \"{installdir}\" }}");
        assert!(
            parse_app_manifest(manifest.as_bytes()).is_err(),
            "reserved device basename must be rejected: {installdir}"
        );
    }
}

#[test]
fn parses_an_escaped_windows_library_path_on_non_windows_test_hosts() {
    let paths = parse_library_folders(br#""libraryfolders" { "0" { "path" "D:\\SteamLibrary" } }"#)
        .unwrap();

    assert_eq!(paths, vec![PathBuf::from(r"D:\SteamLibrary")]);
}

#[test]
fn default_steam_library_is_considered_when_library_metadata_is_absent_or_corrupt() {
    for metadata in [None, Some(b"malformed".as_slice())] {
        let steam = tempfile::tempdir().unwrap();
        let game_root = steam.path().join("steamapps/common/Wuthering Waves");
        let executable = game_root.join("Client/Binaries/Win64").join(EXE_NAME);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"exe").unwrap();
        fs::write(
            steam.path().join("steamapps/appmanifest_3513350.acf"),
            "\"AppState\" { \"appid\" \"3513350\" \"installdir\" \"Wuthering Waves\" }",
        )
        .unwrap();
        if let Some(metadata) = metadata {
            fs::write(steam.path().join("steamapps/libraryfolders.vdf"), metadata).unwrap();
        }
        let provider = StaticProvider {
            steam_roots: vec![steam.path().to_path_buf()],
            ..StaticProvider::default()
        };

        let installations = discover_with_provider(&provider).unwrap();

        assert_eq!(installations.len(), 1);
        assert_eq!(
            installations[0].game_root,
            game_root.canonicalize().unwrap()
        );
    }
}

#[cfg(unix)]
#[test]
fn rejects_a_steam_common_symlink_that_escapes_the_library() {
    use std::os::unix::fs::symlink;

    let steam = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_common = outside.path().join("common");
    let game_root = outside_common.join("Wuthering Waves");
    let executable = game_root.join("Client/Binaries/Win64").join(EXE_NAME);
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"exe").unwrap();
    fs::create_dir_all(steam.path().join("steamapps")).unwrap();
    symlink(&outside_common, steam.path().join("steamapps/common")).unwrap();
    fs::write(
        steam.path().join("steamapps/appmanifest_3513350.acf"),
        "\"AppState\" { \"appid\" \"3513350\" \"installdir\" \"Wuthering Waves\" }",
    )
    .unwrap();
    fs::write(
        steam.path().join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}",
            steam.path().display()
        ),
    )
    .unwrap();
    let provider = StaticProvider {
        steam_roots: vec![steam.path().to_path_buf()],
        ..StaticProvider::default()
    };

    assert!(discover_with_provider(&provider).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn rejects_steam_metadata_reached_through_a_symlinked_parent() {
    use std::os::unix::fs::symlink;

    let steam = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let game_root = outside.path().join("steamapps/common/Wuthering Waves");
    let executable = game_root.join("Client/Binaries/Win64").join(EXE_NAME);
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"exe").unwrap();
    fs::write(
        outside.path().join("steamapps/appmanifest_3513350.acf"),
        "\"AppState\" { \"appid\" \"3513350\" \"installdir\" \"Wuthering Waves\" }",
    )
    .unwrap();
    fs::write(
        outside.path().join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}",
            steam.path().display()
        ),
    )
    .unwrap();
    symlink(
        outside.path().join("steamapps"),
        steam.path().join("steamapps"),
    )
    .unwrap();
    let provider = StaticProvider {
        steam_roots: vec![steam.path().to_path_buf()],
        ..StaticProvider::default()
    };

    assert!(discover_with_provider(&provider).unwrap().is_empty());
}

#[test]
fn deduplicates_the_same_canonical_executable_across_candidate_sources() {
    let fixture = GameFixture::new("Wuthering Waves Game");
    let provider = StaticProvider {
        uninstall_entries: vec![
            UninstallEntry {
                display_name: "Wuthering Waves".into(),
                publisher: Some("Kuro Games".into()),
                install_location: fixture.game_root.clone(),
            },
            UninstallEntry {
                display_name: "Wuthering Waves".into(),
                publisher: Some("Kuro Games".into()),
                install_location: fixture.game_root.clone(),
            },
        ],
        ..StaticProvider::default()
    };

    assert_eq!(discover_with_provider(&provider).unwrap().len(), 1);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn system_discovery_is_empty_on_non_windows() {
    assert!(discover_installations().unwrap().is_empty());
}
