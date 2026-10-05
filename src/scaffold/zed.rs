//! Additive Zed discovery for the optional native library and hidden host.
//! JSONC edits touch individual values, preserving unrelated settings and comments.
use crate::compiled::atomic_write;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn prepare(root: &Path, native: bool) -> Result<()> {
    let path = root.join(".zed/settings.json");
    let mut source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{\n}\n".into(),
        Err(error) => return Err(error.into()),
    };
    let settings: Value =
        serde_json::from_str(&jsonc(&source)?).context("parse existing .zed/settings.json")?;
    let linked_path = [
        "lsp",
        "rust-analyzer",
        "initialization_options",
        "linkedProjects",
    ];
    let mut linked = settings
        .pointer("/lsp/rust-analyzer/initialization_options/linkedProjects")
        .map(|value| {
            value
                .as_array()
                .cloned()
                .context("linkedProjects must be an array")
        })
        .transpose()?
        .unwrap_or_default();
    // Remove only the paths owned by this generator, then rebuild the minimal
    // list. Keep user-linked projects (including rust-project.json objects).
    linked.retain(|value| {
        !matches!(
            value.as_str(),
            Some(
                "./native/Cargo.toml"
                    | "native/Cargo.toml"
                    | "./.revenant/desktop/Cargo.toml"
                    | ".revenant/desktop/Cargo.toml"
            )
        )
    });
    if native {
        linked.push(json!("./native/Cargo.toml"));
    }
    linked.push(json!("./.revenant/desktop/Cargo.toml"));
    set(&mut source, &linked_path, Value::Array(linked))?;
    let mut inclusions = settings
        .get("file_scan_inclusions")
        .map(|value| {
            value
                .as_array()
                .cloned()
                .context("file_scan_inclusions must be an array")
        })
        .transpose()?
        .unwrap_or_else(|| vec![json!("...")]);
    for pattern in [".revenant/desktop/**", ".revenant/sdk/crates/**"] {
        let value = json!(pattern);
        if !inclusions.contains(&value) {
            inclusions.push(value);
        }
    }
    set(
        &mut source,
        &["file_scan_inclusions"],
        Value::Array(inclusions),
    )?;
    // Respect explicit project-panel preferences. In new projects make the
    // linked, gitignored bootstrap visible as well as available to the LSP.
    for key in ["hide_hidden", "hide_gitignore"] {
        if settings.pointer(&format!("/project_panel/{key}")).is_none() {
            set(&mut source, &["project_panel", key], json!(false))?;
        }
    }
    // No file is written until every merge and the resulting JSONC validate.
    let _: Value = serde_json::from_str(&jsonc(&source)?)?;
    std::fs::create_dir_all(path.parent().context("Zed settings parent")?)?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(&source) {
        atomic_write(&path, source.as_bytes())?;
    }
    Ok(())
}

/// Mask comments and trailing commas without changing byte positions. Strings
/// remain untouched, including escaped quotes and URLs that contain `//`.
fn jsonc(source: &str) -> Result<String> {
    let mut bytes = source.as_bytes().to_vec();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        } else if bytes[i..].starts_with(b"/*") {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                if !matches!(bytes[i], b'\r' | b'\n') {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            anyhow::ensure!(i + 1 < bytes.len(), "unterminated JSONC comment");
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            continue;
        }
        i += 1;
    }
    i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
        } else if bytes[i] == b',' {
            let next = bytes[i + 1..]
                .iter()
                .find(|byte| !byte.is_ascii_whitespace());
            if matches!(next, Some(b'}' | b']')) {
                bytes[i] = b' ';
            }
        }
        i += 1;
    }
    Ok(String::from_utf8(bytes)?)
}

/// Recurse through object fields, replacing only the requested leaf. Missing
/// objects are inserted beside the last value, before any existing trailing comma.
fn set(source: &mut String, path: &[&str], value: Value) -> Result<()> {
    let clean = jsonc(source)?;
    let existing: Value = serde_json::from_str(&clean)?;
    anyhow::ensure!(
        existing.is_object(),
        "Zed settings path must contain an object"
    );
    if path.len() == 1 && existing.get(path[0]) == Some(&value) {
        return Ok(());
    }
    let mut offset = clean.find('{').context("settings object")? + 1;
    let mut insertion = offset;
    let mut populated = false;
    loop {
        while clean
            .as_bytes()
            .get(offset)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b',')
        {
            offset += 1;
        }
        if clean.as_bytes().get(offset) == Some(&b'}') {
            break;
        }
        let mut stream = serde_json::Deserializer::from_str(&clean[offset..]).into_iter::<Value>();
        let key = stream.next().context("settings key")??;
        offset += stream.byte_offset();
        while clean
            .as_bytes()
            .get(offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            offset += 1;
        }
        anyhow::ensure!(
            clean.as_bytes().get(offset) == Some(&b':'),
            "settings field separator"
        );
        offset += 1;
        while clean
            .as_bytes()
            .get(offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            offset += 1;
        }
        let start = offset;
        let mut stream = serde_json::Deserializer::from_str(&clean[start..]).into_iter::<Value>();
        stream.next().context("settings value")??;
        offset = start + stream.byte_offset();
        if key.as_str() == Some(path[0]) {
            let replacement = if path.len() == 1 {
                serde_json::to_string_pretty(&value)?
            } else {
                let mut child = source[start..offset].to_owned();
                set(&mut child, &path[1..], value)?;
                child
            };
            source.replace_range(start..offset, &replacement);
            return Ok(());
        }
        insertion = offset;
        populated = true;
    }
    let mut nested = value;
    for key in path[1..].iter().rev() {
        nested = Value::Object(serde_json::Map::from_iter([(key.to_string(), nested)]));
    }
    let field = format!(
        "{}\n  {}: {}",
        if populated { "," } else { "" },
        serde_json::to_string(path[0])?,
        serde_json::to_string_pretty(&nested)?
    );
    source.insert_str(insertion, &field);
    Ok(())
}
