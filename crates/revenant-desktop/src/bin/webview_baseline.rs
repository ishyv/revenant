//! Read-only WebView runtime discovery for disposable clean-machine acceptance.
#[cfg(all(windows, feature = "tauri-host"))]
fn main() {
    let runtime = match tauri::webview_version() {
        Ok(version) => serde_json::json!({"available": true, "version": version}),
        Err(error) => serde_json::json!({"available": false, "error": error.to_string()}),
    };
    println!("{}", runtime);
}

#[cfg(not(all(windows, feature = "tauri-host")))]
fn main() {
    println!("{{\"unsupported\": true, \"reason\": \"Requires Windows and tauri-host\"}}");
}
