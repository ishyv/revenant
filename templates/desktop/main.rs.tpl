//! Generated Revenant desktop bootstrap. Customize native/src/lib.rs instead.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let application = {{application}};
    let development = cfg!(debug_assertions) && std::env::args().any(|argument| argument == "--revenant-dev");
    let options = revenant_desktop::DesktopOptions {
        app_id: {{app_id}}.into(),
        agents_enabled: {{agents_enabled}},
        native_surfaces: {{native_surfaces}},
        development,
        project_root: if development { Some(std::env::current_dir()?) } else { None },
        source_fingerprint: {{source_fingerprint}},
        build_attempt: {{build_attempt}},
        ..Default::default()
    };
    if revenant_desktop::mcp_if_requested(&application, &options)? {
        return Ok(());
    }
    // Contract extraction must finish before a builder or window is created.
    if revenant_desktop::export_contract_if_requested(&application)? {
        return Ok(());
    }
    revenant_desktop::launch(
        tauri::Builder::default(),
        tauri::generate_context!(),
        application,
        options,
    )?;
    Ok(())
}
