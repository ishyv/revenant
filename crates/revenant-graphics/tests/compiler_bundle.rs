#[allow(dead_code)]
#[path = "../src/compiler_bundle.rs"]
mod bundle;

#[cfg(all(feature = "gpu", windows))]
#[path = "../src/compiler.rs"]
mod compiler;

use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn verifies_digest_before_extracting_untrusted_archive() {
    let bytes = archive(&[("bin/x64/dxcompiler.dll", b"compiler")]);
    let out = tempfile::tempdir().unwrap();
    let entry = bundle::Artifact {
        archive_path: "bin/x64/dxcompiler.dll",
        file_name: "dxcompiler.dll",
        sha256: "bad",
        max_bytes: 32,
    };
    assert!(
        bundle::extract_verified(&bytes, "bad", &[entry], out.path())
            .unwrap_err()
            .to_string()
            .contains("archive checksum")
    );
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn extracts_only_pinned_files_and_checks_each_digest() {
    let bytes = archive(&[
        ("bin/x64/dxcompiler.dll", b"compiler"),
        ("../escape.dll", b"ignored"),
        ("bin/x86/dxcompiler.dll", b"wrong arch"),
    ]);
    let out = tempfile::tempdir().unwrap();
    let expected = digest(b"compiler");
    let entry = bundle::Artifact {
        archive_path: "bin/x64/dxcompiler.dll",
        file_name: "dxcompiler.dll",
        sha256: &expected,
        max_bytes: 32,
    };
    bundle::extract_verified(&bytes, &digest(&bytes), &[entry], out.path()).unwrap();
    assert_eq!(
        std::fs::read(out.path().join("dxcompiler.dll")).unwrap(),
        b"compiler"
    );
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 1);
}

#[test]
fn rejects_oversized_entry_before_writing() {
    let bytes = archive(&[("bin/x64/dxcompiler.dll", b"too large")]);
    let out = tempfile::tempdir().unwrap();
    let entry = bundle::Artifact {
        archive_path: "bin/x64/dxcompiler.dll",
        file_name: "dxcompiler.dll",
        sha256: "unused",
        max_bytes: 2,
    };
    assert!(
        bundle::extract_verified(&bytes, &digest(&bytes), &[entry], out.path())
            .unwrap_err()
            .to_string()
            .contains("size limit")
    );
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn a_bad_dll_digest_leaves_no_partial_bundle() {
    let bytes = archive(&[
        ("bin/x64/dxcompiler.dll", b"compiler"),
        ("bin/x64/dxil.dll", b"validator"),
    ]);
    let out = tempfile::tempdir().unwrap();
    let expected = digest(b"compiler");
    let entries = [
        bundle::Artifact {
            archive_path: "bin/x64/dxcompiler.dll",
            file_name: "dxcompiler.dll",
            sha256: &expected,
            max_bytes: 32,
        },
        bundle::Artifact {
            archive_path: "bin/x64/dxil.dll",
            file_name: "dxil.dll",
            sha256: "bad",
            max_bytes: 32,
        },
    ];
    assert!(
        bundle::extract_verified(&bytes, &digest(&bytes), &entries, out.path())
            .unwrap_err()
            .to_string()
            .contains("dxil.dll checksum")
    );
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}
