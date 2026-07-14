use wuwa_ini_tool_lib::ini_document::{IniDocument, IniError, ManagedChange, SemanticChange};

#[test]
fn untouched_utf8_crlf_document_round_trips_byte_for_byte() {
    let bytes = include_bytes!("fixtures/utf8-crlf.ini");
    let document = IniDocument::parse(bytes).unwrap();

    assert_eq!(document.as_bytes(), bytes);
}

#[test]
fn untouched_utf8_bom_lf_document_round_trips_byte_for_byte() {
    let bytes = include_bytes!("fixtures/utf8-bom-lf.ini");
    let document = IniDocument::parse(bytes).unwrap();

    assert_eq!(document.as_bytes(), bytes);
}

#[test]
fn untouched_utf16le_crlf_document_round_trips_byte_for_byte() {
    let bytes = include_bytes!("fixtures/utf16le-crlf.ini");
    let document = IniDocument::parse(bytes).unwrap();

    assert_eq!(document.as_bytes(), bytes);
}

#[test]
fn merge_changes_only_the_managed_key() {
    let bytes = b"; keep\r\n[SystemSettings]\r\nr.Foo=1\r\ncustom=stay\r\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.Foo", "2")])
        .unwrap();

    assert_eq!(preview.before, bytes);
    assert_eq!(
        preview.after,
        b"; keep\r\n[SystemSettings]\r\nr.Foo=2\r\ncustom=stay\r\n"
    );
    assert_eq!(
        preview.semantic_changes,
        vec![SemanticChange {
            section: "SystemSettings".into(),
            key: "r.Foo".into(),
            before: Some("1".into()),
            after: Some("2".into()),
        }]
    );
}

#[test]
fn merge_preserves_key_spelling_spacing_inline_comment_and_lf() {
    let bytes = b"# heading\n[ systemsettings ]\n  R.FOO = 1  ; keep this\nOther=stay\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.foo", "7")])
        .unwrap();

    assert_eq!(
        preview.after,
        b"# heading\n[ systemsettings ]\n  R.FOO = 7  ; keep this\nOther=stay\n"
    );
}

#[test]
fn merge_preserves_an_inline_comment_after_an_empty_value() {
    let bytes = b"[SystemSettings]\nr.Foo=  ; keep empty-value comment\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.Foo", "7")])
        .unwrap();

    assert_eq!(
        preview.after,
        b"[SystemSettings]\nr.Foo=7  ; keep empty-value comment\n"
    );
    assert_eq!(preview.semantic_changes[0].before.as_deref(), Some(""));
}

#[test]
fn merge_finds_a_key_in_a_repeated_section() {
    let bytes = b"[SystemSettings]\nfirst=stay\n[Other]\nx=1\n[ systemsettings ]\nr.Foo=1\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SYSTEMSETTINGS", "R.FOO", "2")])
        .unwrap();

    assert_eq!(
        preview.after,
        b"[SystemSettings]\nfirst=stay\n[Other]\nx=1\n[ systemsettings ]\nr.Foo=2\n"
    );
}

#[test]
fn managed_section_does_not_trim_non_ascii_whitespace() {
    let bytes = b"[SystemSettings]\nr.Foo=1\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set(
            "\u{a0}SystemSettings\u{a0}",
            "r.Foo",
            "2",
        )])
        .unwrap();

    assert_eq!(
        String::from_utf8(preview.after).unwrap(),
        "[SystemSettings]\nr.Foo=1\n[\u{a0}SystemSettings\u{a0}]\nr.Foo=2\n"
    );
}

#[test]
fn managed_key_does_not_trim_non_ascii_whitespace() {
    let bytes = b"[SystemSettings]\nr.Foo=1\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set(
            "SystemSettings",
            "\u{a0}r.Foo\u{a0}",
            "2",
        )])
        .unwrap();

    assert_eq!(
        String::from_utf8(preview.after).unwrap(),
        "[SystemSettings]\nr.Foo=1\n\u{a0}r.Foo\u{a0}=2\n"
    );
}

#[test]
fn duplicate_managed_keys_across_repeated_sections_are_ambiguous() {
    let bytes = b"[SystemSettings]\nr.Foo=1\n[ systemsettings ]\n R.FOO =2\n";
    let error = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.Foo", "3")])
        .unwrap_err();

    assert!(matches!(
        error,
        IniError::AmbiguousManagedKey { section, key }
            if section == "SystemSettings" && key == "r.Foo"
    ));
}

#[test]
fn deletion_removes_only_the_managed_line() {
    let bytes = b"[SystemSettings]\r\nkeep=1\r\nr.Foo=2\r\n; tail\r\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::delete("SystemSettings", "r.Foo")])
        .unwrap();

    assert_eq!(preview.after, b"[SystemSettings]\r\nkeep=1\r\n; tail\r\n");
    assert_eq!(preview.semantic_changes[0].before.as_deref(), Some("2"));
    assert_eq!(preview.semantic_changes[0].after, None);
}

#[test]
fn set_inserts_key_and_missing_section_using_existing_line_ending() {
    let bytes = b"; keep\r\n[Other]\r\nx=1\r\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.Foo", "2")])
        .unwrap();

    assert_eq!(
        preview.after,
        b"; keep\r\n[Other]\r\nx=1\r\n[SystemSettings]\r\nr.Foo=2\r\n"
    );
    assert_eq!(preview.semantic_changes[0].before, None);
    assert_eq!(preview.semantic_changes[0].after.as_deref(), Some("2"));
}

#[test]
fn set_inserts_a_missing_key_at_the_end_of_an_existing_section() {
    let bytes = b"[SystemSettings]\nkeep=1\n[Other]\nx=1\n";
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.Foo", "2")])
        .unwrap();

    assert_eq!(
        preview.after,
        b"[SystemSettings]\nkeep=1\nr.Foo=2\n[Other]\nx=1\n"
    );
}

#[test]
fn utf16le_merge_preserves_the_bom_and_encoding() {
    let bytes = include_bytes!("fixtures/utf16le-crlf.ini");
    let preview = IniDocument::parse(bytes)
        .unwrap()
        .merge(&[ManagedChange::set("SystemSettings", "r.Foo", "9")])
        .unwrap();

    assert!(preview.after.starts_with(&[0xff, 0xfe]));
    let decoded = String::from_utf16(
        &preview.after[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(decoded, "; UTF-16LE\r\n[SystemSettings]\r\nr.Foo=9\r\n");
}

#[test]
fn invalid_or_unsupported_encodings_are_rejected() {
    for bytes in [
        &[0xff, 0xfe, 0x41][..],
        &[0xfe, 0xff, 0x00, 0x41][..],
        &[0x80, 0x81][..],
        &[b'[', 0, b'X', 0, b']', 0][..],
    ] {
        assert!(matches!(
            IniDocument::parse(bytes),
            Err(IniError::UnsupportedEncoding)
        ));
    }
}

#[test]
fn utf32le_bom_is_rejected() {
    let bytes = [0xff, 0xfe, 0x00, 0x00, b'[', 0x00, 0x00, 0x00];

    assert!(matches!(
        IniDocument::parse(&bytes),
        Err(IniError::UnsupportedEncoding)
    ));
}

#[test]
fn utf32be_bom_is_rejected() {
    let bytes = [0x00, 0x00, 0xfe, 0xff, 0x00, 0x00, 0x00, b'['];

    assert!(matches!(
        IniDocument::parse(&bytes),
        Err(IniError::UnsupportedEncoding)
    ));
}
