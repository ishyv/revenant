//! Embed a curated source snapshot so generated apps do not depend on this checkout.
//!
//! Build outputs, dependency installations, dotfiles and symlinks are excluded.
//! Native crate build scripts and permission manifests travel with source files;
//! desktop templates and icons are embedded by the ordinary CLI source modules.
use std::{env, fs, path::Path};

fn collect(root: &Path, relative: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    let path = root.join(relative);
    println!("cargo:rerun-if-changed={}", path.display());
    let Ok(entries) = fs::read_dir(&path) else {
        return;
    };
    let mut entries: Vec<_> = entries.map(|e| e.expect("read SDK directory")).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let ty = entry.file_type().expect("SDK entry type");
        // Symlinks cannot pull files outside the curated source roots.
        if ty.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.')
            || matches!(
                name.to_str(),
                Some(
                    "target"
                        | "node_modules"
                        | "dist"
                        | "build"
                        | "tests"
                        | "examples"
                        | "benches"
                        | "docs"
                )
            )
        {
            continue;
        }
        let child = relative.join(name);
        if ty.is_dir() {
            collect(root, &child, files);
        } else if matches!(
            child.extension().and_then(|x| x.to_str()),
            Some(
                "rs" | "ts"
                    | "svelte"
                    | "css"
                    | "js"
                    | "json"
                    | "toml"
                    | "sql"
                    | "html"
                    | "svg"
                    | "png"
                    | "ico"
                    | "icns"
                    | "md"
            )
        ) {
            files.push((
                child.to_string_lossy().replace('\\', "/"),
                fs::read(entry.path()).expect("read SDK source"),
            ));
        }
    }
}

fn main() {
    let root = std::path::PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let mut files = Vec::new();
    for base in [
        "crates/revenant-core",
        "crates/revenant-sdk",
        "crates/revenant-macros",
        "crates/revenant-desktop",
        "packages/client",
    ] {
        println!("cargo:rerun-if-changed={base}");
        // Collect the curated root recursively: a new host module, permission,
        // build helper or asset must travel with the CLI without a second list.
        assert!(root.join(base).is_dir(), "missing SDK source root {base}");
        collect(&root, Path::new(base), &mut files);
    }
    let archive = "vendor/hyvui-1.0.0.tgz";
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed={archive}");
    if let Ok(bytes) = fs::read(root.join(archive)) {
        files.push((archive.to_owned(), bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut source = String::from("pub static EMBEDDED: &[(&str, &[u8])] = &[\n");
    for (name, bytes) in files {
        source.push_str(&format!("({name:?}, &{bytes:?}),\n"));
    }
    source.push_str("];\n");
    fs::write(
        std::path::PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("sdk_sources.rs"),
        source,
    )
    .expect("write embedded SDK");
}
