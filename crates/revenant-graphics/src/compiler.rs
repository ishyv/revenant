// Windows compiler files owned for the lifetime of the GPU renderer.
use revenant_core::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs;

include!(concat!(env!("OUT_DIR"), "/dxc_manifest.rs"));

/// Must be dropped after the surface, device, queue, and their dependent objects.
pub(crate) struct BundledDxc {
    compiler_path: String,
    _directory: tempfile::TempDir,
}

impl BundledDxc {
    pub(crate) fn prepare() -> Result<Self> {
        let prepare = || -> std::io::Result<Self> {
            let directory = tempfile::Builder::new().prefix("revenant-dxc-").tempdir()?;
            for (name, bytes, expected) in [
                (
                    "dxcompiler.dll",
                    include_bytes!(concat!(env!("OUT_DIR"), "/dxcompiler.dll")).as_slice(),
                    DXCOMPILER_SHA256,
                ),
                (
                    "dxil.dll",
                    include_bytes!(concat!(env!("OUT_DIR"), "/dxil.dll")).as_slice(),
                    DXIL_SHA256,
                ),
            ] {
                if format!("{:x}", Sha256::digest(bytes)) != expected {
                    return Err(std::io::Error::other(format!(
                        "Embedded {name} checksum mismatch"
                    )));
                }
                fs::write(directory.path().join(name), bytes)?;
            }
            for (name, notice) in [
                (
                    "LICENCE-MIT.txt",
                    include_str!("../assets/dxc/LICENCE-MIT.txt"),
                ),
                (
                    "LICENSE-LLVM.txt",
                    include_str!("../assets/dxc/LICENSE-LLVM.txt"),
                ),
                (
                    "LICENSE-MS.txt",
                    include_str!("../assets/dxc/LICENSE-MS.txt"),
                ),
                ("PROVENANCE.md", include_str!("../assets/dxc/PROVENANCE.md")),
            ] {
                fs::write(directory.path().join(name), notice)?;
            }
            let absolute = fs::canonicalize(directory.path().join("dxcompiler.dll"))?;
            let compiler_path = absolute
                .to_str()
                .ok_or_else(|| {
                    std::io::Error::other(
                        "The compiler directory cannot be represented by wgpu's UTF-8 path API",
                    )
                })?
                .to_owned();
            Ok(Self {
                compiler_path,
                _directory: directory,
            })
        };
        prepare().map_err(|error| {
            Error::new(
                "native_gpu_compiler",
                format!("Cannot prepare the bundled shader compiler: {error}"),
            )
        })
    }

    pub(crate) fn compiler(&self) -> wgpu::Dx12Compiler {
        wgpu::Dx12Compiler::DynamicDxc {
            dxc_path: self.compiler_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_has_absolute_compiler_path_and_private_lifetime() {
        let bundle = BundledDxc::prepare().unwrap();
        let wgpu::Dx12Compiler::DynamicDxc { dxc_path } = bundle.compiler() else {
            panic!("Explicit bundled DXC expected")
        };
        let path = std::path::PathBuf::from(dxc_path);
        assert!(path.is_absolute());
        assert!(path.is_file());
        assert!(path.with_file_name("dxil.dll").is_file());
        assert!(path.with_file_name("LICENSE-MS.txt").is_file());
        let other = BundledDxc::prepare().unwrap();
        assert_ne!(
            format!("{:?}", other.compiler()),
            format!("{:?}", bundle.compiler())
        );
        drop(bundle);
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    #[ignore = "Requires a Windows DX12 adapter; run explicitly on the validation host"]
    fn bundled_compiler_builds_a_dx12_pipeline() {
        pollster::block_on(async {
            let compiler = BundledDxc::prepare().unwrap();
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                backend_options: wgpu::BackendOptions {
                    dx12: wgpu::Dx12BackendOptions {
                        shader_compiler: compiler.compiler(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            eprintln!("Bundled DXC adapter: {}", adapter.get_info().name);
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Bundled DXC acceptance"),
                source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Bundled DXC acceptance"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            assert!(scope.pop().await.is_none());
            drop(pipeline);
            drop(shader);
            drop(queue);
            drop(device);
            drop(adapter);
            drop(instance);
            let path = std::path::PathBuf::from(compiler.compiler_path.clone());
            drop(compiler);
            assert!(!path.parent().unwrap().exists());
        });
    }
}
