//! File-backed templates embedded into the CLI at compile time.
//!
//! Keep author-facing examples here as ordinary reviewable files. Tokens use
//! double braces so Rust, TOML, JSON, and Svelte syntax remains readable.

/// Templates for the minimal handwritten app; no native source is required.
pub const APP: &[(&str, &str)] = &[
    (
        "revenant.toml",
        include_str!("../../templates/app/revenant.toml.tpl"),
    ),
    (
        ".gitignore",
        include_str!("../../templates/app/gitignore.tpl"),
    ),
    (
        "GETTING_STARTED.md",
        include_str!("../../templates/app/GETTING_STARTED.md.tpl"),
    ),
    (
        "web/package.json",
        include_str!("../../templates/app/web/package.json.tpl"),
    ),
    (
        "web/svelte.config.js",
        include_str!("../../templates/app/web/svelte.config.js.tpl"),
    ),
    (
        "web/vite.config.js",
        include_str!("../../templates/app/web/vite.config.js.tpl"),
    ),
    (
        "web/tsconfig.json",
        include_str!("../../templates/app/web/tsconfig.json.tpl"),
    ),
    (
        "web/src/app.html",
        include_str!("../../templates/app/web/app.html.tpl"),
    ),
    (
        "web/src/routes/+layout.ts",
        include_str!("../../templates/app/web/layout.ts.tpl"),
    ),
    (
        "web/src/routes/+layout.svelte",
        include_str!("../../templates/app/web/layout.svelte.tpl"),
    ),
    (
        "web/src/routes/+page.svelte",
        include_str!("../../templates/app/web/page.svelte.tpl"),
    ),
];
/// Generated host manifest, sharing one SDK with the optional native library.
pub const HOST_CARGO: &str = include_str!("../../templates/desktop/Cargo.toml.tpl");
/// Export-first desktop bootstrap.
pub const HOST_MAIN: &str = include_str!("../../templates/desktop/main.rs.tpl");
/// Tauri's ordinary build integration.
pub const HOST_BUILD: &str = include_str!("../../templates/desktop/build.rs.tpl");

/// Native platform icons required by Tauri code generation and bundlers.
pub const HOST_ASSETS: &[(&str, &[u8])] = &[
    (
        "icons/icon.png",
        include_bytes!("../../templates/desktop/icons/icon.png"),
    ),
    (
        "icons/icon.ico",
        include_bytes!("../../templates/desktop/icons/icon.ico"),
    ),
    (
        "icons/icon.icns",
        include_bytes!("../../templates/desktop/icons/icon.icns"),
    ),
];

/// Substitute only named tokens, leaving language punctuation untouched.
pub fn render(template: &str, tokens: &[(&str, &str)]) -> String {
    // Scan the template once. Replacement contents (including authored Rustdoc
    // and JSON) must never be interpreted as further template instructions.
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        output.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let Some(end) = rest.find("}}") else {
            output.push_str("{{");
            output.push_str(rest);
            return output;
        };
        let name = &rest[..end];
        if let Some((_, replacement)) = tokens.iter().find(|(token, _)| *token == name) {
            output.push_str(replacement);
        } else {
            output.push_str("{{");
            output.push_str(name);
            output.push_str("}}");
        }
        rest = &rest[end + 2..];
    }
    output.push_str(rest);
    output
}
