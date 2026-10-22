use revenant::{config::RevenantConfig, scaffold};

#[test]
fn surfaces_are_an_explicit_optional_host_capability() {
    let temp = tempfile::tempdir().unwrap();
    scaffold::create_project(temp.path(), "surface-app").unwrap();
    std::fs::write(
        temp.path().join("revenant.toml"),
        "version = 3\n[project]\nname = 'surface-app'\n[desktop]\nnative_surfaces = true\n",
    )
    .unwrap();
    let config =
        RevenantConfig::load(temp.path()).expect("surface configuration must be supported");
    scaffold::prepare_host(temp.path(), &config).unwrap();
    let cargo = std::fs::read_to_string(temp.path().join(".revenant/desktop/Cargo.toml")).unwrap();
    assert!(cargo.contains("native-surfaces"));
}
