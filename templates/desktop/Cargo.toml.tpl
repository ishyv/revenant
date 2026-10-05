[package]
name = "{{project_name}}"
version = "0.3.0"
edition = "2024"
build = "build.rs"

# Keep generated hosts independent of any surrounding repository workspace.
[workspace]

[features]
default = ["desktop"]
desktop = []
custom-protocol = ["tauri/custom-protocol"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
# Tauri discovers plugin permissions from direct host dependencies.
tauri-plugin-dialog = "2"
revenant = { package = "revenant-sdk", version = "0.3.0", path = "../sdk/crates/revenant-sdk" }
revenant-desktop = { version = "0.3.0", path = "../sdk/crates/revenant-desktop" }
{{native_dependency}}
