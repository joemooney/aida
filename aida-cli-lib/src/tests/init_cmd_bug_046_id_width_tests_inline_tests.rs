// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-046.acbed48e | ai:claude
#[test]
fn id_width_is_written_to_ids_table_preserving_other_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "# keep me\n[project]\nname = \"x\"\n",
    )
    .unwrap();
    super::write_ids_widths(dir.path(), Some(4), Some(4)).unwrap();
    let body = std::fs::read_to_string(dir.path().join(".aida/config.toml")).unwrap();
    assert!(body.contains("# keep me"), "{body}");
    let w = aida_core::id_identity::IdWidths::from_config_toml(&body);
    assert_eq!((w.seq_width, w.node_seq_width), (Some(4), Some(4)));
    // `aida config digits 0` sets only the legacy width.
    super::write_ids_widths(dir.path(), Some(0), None).unwrap();
    let body = std::fs::read_to_string(dir.path().join(".aida/config.toml")).unwrap();
    let w = aida_core::id_identity::IdWidths::from_config_toml(&body);
    assert_eq!((w.seq_width, w.node_seq_width), (Some(0), Some(4)));
}
