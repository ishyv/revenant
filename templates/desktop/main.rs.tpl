//! Generated Revenant desktop bootstrap. Customize native/src/lib.rs instead.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let application = {{application}};
    // Contract extraction must finish before a builder or window is created.
    if revenant_desktop::export_contract_if_requested(&application)? {
        return Ok(());
    }
    revenant_desktop::launch(
        tauri::Builder::default(),
        tauri::generate_context!(),
        application,
        revenant_desktop::DesktopOptions {
            app_id: {{app_id}}.into(),
            ..Default::default()
        },
    )?;
    Ok(())
}
