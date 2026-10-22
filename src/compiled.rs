//! Native compiled-contract publication and documented TypeScript bindings.
//!
//! Extraction runs the same executable used by the desktop window with the
//! private `--revenant-contract` flag. Every generation is immutable; a single
//! atomic facade-pointer replacement publishes it. A failed native compilation,
//! invalid manifest, or interrupted export cannot replace the previous facade.
use crate::toolchain::process;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    protocol: u32,
    digest: String,
    operations: Vec<Operation>,
}
#[derive(Deserialize)]
struct Operation {
    id: String,
    description: String,
    input: Definition,
    output: Definition,
    updates: Option<Definition>,
    source: Option<OperationSource>,
    arguments: Vec<String>,
    effect: String,
    independent: bool,
    agent: bool,
}
#[derive(Deserialize)]
struct Definition {
    typescript: String,
    schema: Value,
}
#[derive(Deserialize)]
struct OperationSource {
    file: String,
    line: u32,
    module: String,
}

/// Private staging area for one compiled native manifest and generated facade.
/// Dropping it removes only unpublished files owned by this invocation.
pub struct BuildStage {
    root: PathBuf,
    pub directory: PathBuf,
    source_identity: Option<String>,
    build_attempt: Option<String>,
}
impl BuildStage {
    /// Allocate staging beside published generations, on the same filesystem.
    pub fn new(root: &Path) -> Result<Self> {
        Self::new_with_source_identity(
            root,
            crate::inspection::latest_build_source_fingerprint(root)?,
            None,
        )
    }

    /// Capture the exact precompile identity, independent of later build attempts.
    pub fn new_with_source_identity(
        root: &Path,
        source_identity: Option<String>,
        build_attempt: Option<String>,
    ) -> Result<Self> {
        let root = dunce::canonicalize(root)?;
        crate::scaffold::sdk::check(&root)?;
        let base = root.join("web/src/lib/.revenant");
        std::fs::create_dir_all(&base)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = base.join(format!(".stage-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&directory)?;
        Ok(Self {
            root,
            directory,
            source_identity,
            build_attempt,
        })
    }

    /// Export without launching an app window, validate, then publish atomically.
    /// Stdout must contain exactly one manifest; logging belongs on stderr.
    pub fn publish(&self, executable: &Path, verbose: bool) -> Result<String> {
        let outcome = self.publish_inner(executable, verbose);
        if let Err(error) = &outcome
            && self.root.join(".revenant/inspection/build.json").is_file()
        {
            crate::inspection::record_build_finish(
                &self.root,
                false,
                vec![crate::inspection::Diagnostic::error(
                    "contract",
                    "contract_publication_failed",
                    format!("{error:#}"),
                )],
            )?;
        }
        outcome
    }

    fn publish_inner(&self, executable: &Path, verbose: bool) -> Result<String> {
        crate::scaffold::sdk::check(&self.root)?;
        let json = process::run_capture_data(
            &self.root,
            &executable.to_string_lossy(),
            &["--revenant-contract"],
            verbose,
        )
        .context("export compiled desktop contract (before window startup)")?;
        let manifest: Manifest = serde_json::from_str(&json)
            .context("native --revenant-contract must print only manifest JSON")?;
        anyhow::ensure!(
            manifest.version == revenant_core::CONTRACT_MANIFEST_VERSION
                && manifest.protocol == revenant_core::BRIDGE_PROTOCOL_VERSION,
            "compiled desktop contract requires version 6 / protocol 5; regenerate the SDK snapshot with Revenant 0.4"
        );
        anyhow::ensure!(
            manifest.digest.starts_with("sha256:")
                && manifest.digest.len() == 71
                && manifest.digest[7..].bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid compiled contract digest"
        );
        let template = generate_facade(&manifest, &json, "__REVENANT_GENERATION__", &self.root)?;
        let mut hash = Sha256::new();
        hash.update(json.as_bytes());
        hash.update(template.as_bytes());
        if let Some(identity) = &self.source_identity {
            hash.update(identity.as_bytes());
        }
        let generation = format!("{:x}", hash.finalize());
        let facade = generate_facade(&manifest, &json, &generation, &self.root)?;
        std::fs::write(self.directory.join("facade.ts"), facade)?;
        std::fs::write(self.directory.join("revenant.contract.json"), &json)?;
        std::fs::write(
            self.directory.join("generation.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"version":revenant_core::CONTRACT_MANIFEST_VERSION,"protocol":revenant_core::BRIDGE_PROTOCOL_VERSION,"sdkRevision":revenant_core::SDK_COMPATIBILITY_REVISION,"digest":manifest.digest,"generation":generation,
                    "sourceFingerprint":self.source_identity,"buildAttempt":self.build_attempt}),
            )?,
        )?;
        let destination = self
            .directory
            .parent()
            .context("stage parent")?
            .join(&generation);
        if !destination.exists() {
            std::fs::rename(&self.directory, &destination)
                .context("publish immutable native contract generation")?;
        }
        let entry = format!(
            "// Generated from compiled Rust; do not edit.\nexport * from './.revenant/{generation}/facade.ts';\nexport {{ default }} from './.revenant/{generation}/facade.ts';\n"
        );
        crate::scaffold::sdk::check(&self.root)?;
        atomic_write(&self.root.join("web/src/lib/revenant.ts"), entry.as_bytes())?;
        Ok(generation)
    }
}
impl Drop for BuildStage {
    fn drop(&mut self) {
        // This path was allocated by new(); it never names a published generation.
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn generate_facade(
    manifest: &Manifest,
    json: &str,
    generation: &str,
    root: &Path,
) -> Result<String> {
    let digest = serde_json::to_string(&manifest.digest)?;
    let epoch = serde_json::to_string(generation)?;
    let mut output = crate::scaffold::templates::render(
        include_str!("../templates/facade/header.ts.tpl"),
        &[
            ("manifest", json.trim()),
            ("digest", &digest),
            ("generation", &epoch),
        ],
    );
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::from([
        "App".into(),
        "Operation".into(),
        "OperationFunction".into(),
        "ReadOperationFunction".into(),
        "ContractManifest".into(),
        "Application".into(),
        "RuntimeApp".into(),
    ]);
    let mut operations = BTreeMap::<String, BindingNode>::new();
    let mut callables = BTreeMap::<String, BindingNode>::new();
    let mut callable_types = BTreeMap::<String, BindingNode>::new();
    let mut types = BTreeMap::<String, BindingNode>::new();
    let mut files = BTreeMap::<String, BindingNode>::new();
    let mut file_types = BTreeMap::<String, BindingNode>::new();
    let mut media = BTreeMap::<String, BindingNode>::new();
    let mut media_types = BTreeMap::<String, BindingNode>::new();
    let mut checksum_type = String::from("unknown");
    let mut metadata_type = String::from("unknown");
    for (index, op) in manifest.operations.iter().enumerate() {
        anyhow::ensure!(
            matches!(op.effect.as_str(), "unknown" | "readOnly" | "write")
                && op.arguments.iter().all(|name| !name.is_empty())
                && op.arguments.iter().collect::<BTreeSet<_>>().len() == op.arguments.len(),
            "invalid execution metadata for {}",
            op.id
        );
        let _execution_flags = (op.independent, op.agent);
        anyhow::ensure!(
            !op.id.is_empty() && ids.insert(&op.id),
            "duplicate or empty compiled operation id"
        );
        let mut name = namespace(&op.id);
        if !names.insert(name.clone()) {
            name.push_str(&format!("_{:x}", Sha256::digest(op.id.as_bytes()))[..9]);
            anyhow::ensure!(
                names.insert(name.clone()),
                "generated operation namespace collision"
            );
        }
        let docs = operation_docs(op, root);
        output.push_str(&format!("{docs}export namespace {name} {{\n"));
        for (direction, definition) in [("Input", &op.input), ("Output", &op.output)]
            .into_iter()
            .chain(
                op.updates
                    .as_ref()
                    .map(|definition| ("Updates", definition)),
            )
        {
            anyhow::ensure!(
                !definition.typescript.trim().is_empty()
                    && (definition.schema.is_object() || definition.schema.is_boolean()),
                "invalid {} {direction} contract",
                op.id
            );
            // ts-rs owns declarations and their field/type JSDoc. Never parse Rust AST.
            output.push_str(&format!("/** Compiled {direction} contract for `{}`. */\nexport namespace {direction} {{\n{}\n}}\n", op.id, definition.typescript));
        }
        output.push_str("}\n");
        let input = format!("{name}.Input.ContractValue");
        let result = format!("{name}.Output.ContractValue");
        let generic_types = if op.updates.is_some() {
            format!("{input}, {result}, {name}.Updates.ContractValue")
        } else {
            format!("{input}, {result}")
        };
        match op.id.as_str() {
            "files.checksum" => checksum_type = result.clone(),
            "media.readMetadata" => metadata_type = result.clone(),
            _ => {}
        }
        let path: Vec<_> = op.id.split('.').collect();
        insert_binding(
            &mut callables,
            &path,
            format!(
                "callableOperation(runtime.bindOperation<{generic_types}>(contract.operations[{index}]), {})",
                op.effect == "readOnly"
            ),
            docs.clone(),
        )?;
        let callable_type = if op.effect == "readOnly" {
            "ReadOperationFunction"
        } else {
            "OperationFunction"
        };
        insert_binding(
            &mut callable_types,
            &path,
            format!("{callable_type}<{generic_types}>"),
            docs.clone(),
        )?;
        // Compiled built-ins extend the capability objects without replacing
        // their native picker/preview methods. Custom operations remain separate.
        let (bindings, declarations, binding_path) = match op.id.as_str() {
            "files.checksum" => (&mut files, &mut file_types, &path[1..]),
            "media.readMetadata" => (&mut media, &mut media_types, &path[1..]),
            _ => (&mut operations, &mut types, path.as_slice()),
        };
        insert_binding(
            bindings,
            binding_path,
            format!("runtime.bindOperation<{generic_types}>(contract.operations[{index}])"),
            docs.clone(),
        )?;
        insert_binding(
            declarations,
            binding_path,
            format!("Operation<{generic_types}>"),
            docs,
        )?;
    }
    output.push_str(&crate::scaffold::templates::render(
        include_str!("../templates/facade/runtime.ts.tpl"),
        &[
            ("checksum_type", &checksum_type),
            ("metadata_type", &metadata_type),
            ("file_types", &render_bindings(&file_types)?),
            ("media_types", &render_bindings(&media_types)?),
            ("operation_types", &render_bindings(&types)?),
            ("files", &render_bindings(&files)?),
            ("media", &render_bindings(&media)?),
            ("operations", &render_bindings(&operations)?),
            ("callables", &render_bindings(&callables)?),
            ("callable_types", &render_bindings(&callable_types)?),
        ],
    ));
    Ok(output)
}

fn namespace(id: &str) -> String {
    let mut name = String::new();
    let mut upper = true;
    for c in id.chars() {
        if !c.is_ascii_alphanumeric() {
            upper = true;
            continue;
        }
        name.push(if upper { c.to_ascii_uppercase() } else { c });
        upper = false;
    }
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert_str(0, "Operation");
    }
    name
}

fn operation_docs(op: &Operation, root: &Path) -> String {
    let mut text = op.description.replace("*/", "* /");
    if let Some(source) = &op.source {
        let file = source.file.replace('\\', "/");
        // Macro paths are relative to the compiling package, not the generation.
        let relative = if let Ok(relative) = Path::new(&file).strip_prefix(root) {
            relative.to_string_lossy().replace('\\', "/")
        } else if file.starts_with("native/") || file.starts_with(".revenant/") {
            file.clone()
        } else if file.starts_with("crates/") {
            format!(".revenant/sdk/{file}")
        } else if source.module.starts_with("revenant_sdk") {
            format!(".revenant/sdk/crates/revenant-sdk/{file}")
        } else {
            format!("native/{file}")
        };
        let safe = relative
            .replace(' ', "%20")
            .replace(')', "%29")
            .replace('(', "%28")
            .replace("*/", "*%2F");
        text.push_str(&format!(
            "\n\n[Rust source](../../../../../{safe}#L{})\n@see {}",
            source.line,
            source.module.replace("*/", "* /")
        ));
    }
    if text.trim().is_empty() {
        text = format!("Native operation `{}`.", op.id);
    }
    format!(
        "/**\n{}\n */\n",
        text.lines()
            .map(|l| format!(" * {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

enum BindingNode {
    Group(BTreeMap<String, BindingNode>),
    Operation { expression: String, docs: String },
}
fn insert_binding(
    tree: &mut BTreeMap<String, BindingNode>,
    path: &[&str],
    expression: String,
    docs: String,
) -> Result<()> {
    anyhow::ensure!(
        !path.is_empty()
            && !path[0].is_empty()
            && path[0]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            && !matches!(path[0], "__proto__" | "prototype" | "constructor"),
        "invalid operation group"
    );
    if path.len() == 1 {
        anyhow::ensure!(
            !tree.contains_key(path[0]),
            "operation/group name collision"
        );
        tree.insert(path[0].into(), BindingNode::Operation { expression, docs });
    } else {
        let node = tree
            .entry(path[0].into())
            .or_insert_with(|| BindingNode::Group(BTreeMap::new()));
        let BindingNode::Group(group) = node else {
            anyhow::bail!("operation/group name collision");
        };
        insert_binding(group, &path[1..], expression, docs)?;
    }
    Ok(())
}
fn render_bindings(tree: &BTreeMap<String, BindingNode>) -> Result<String> {
    let mut output = String::new();
    for (name, node) in tree {
        if let BindingNode::Operation { docs, .. } = node {
            output.push_str(docs);
        }
        output.push_str(&format!("{}: ", serde_json::to_string(name)?));
        match node {
            BindingNode::Operation { expression, .. } => output.push_str(expression),
            BindingNode::Group(group) => {
                output.push_str(&format!("{{\n{}\n}}", render_bindings(group)?))
            }
        }
        output.push_str(",\n");
    }
    Ok(output)
}

/// Durably replace one artifact on the same filesystem, including on Windows.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = path.with_extension(format!("revenant-stage-{}-{stamp}", std::process::id()));
    struct TemporaryFile(PathBuf);
    impl Drop for TemporaryFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = TemporaryFile(temporary.clone());
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    let result = {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            use windows_sys::Win32::Storage::FileSystem::*;
            if MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            ) == 0
            {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    };
    #[cfg(not(windows))]
    let result = std::fs::rename(&temporary, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("atomically publish {}", path.display()))
}
