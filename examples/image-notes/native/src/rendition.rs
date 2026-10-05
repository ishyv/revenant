//! Root-contained image resolution, bounded decoding and persistent JPEG caching.
use crate::{
    catalog::Catalog,
    contracts::{RenderInput, Rendition},
    error::problem,
    filesystem::{is_link, modified},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageDecoder, ImageReader, codecs::jpeg::JpegEncoder};
use revenant::{Error, Result, TaskContext};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_ENCODED_IMAGE: usize = 700 * 1024;

/// Load a cache entry or generate it after validating the original's current location.
pub(crate) fn render(
    catalog: &Catalog,
    input: &RenderInput,
    context: &TaskContext,
) -> Result<Rendition> {
    let path = resolve_original(catalog, &input.id)?;
    let bound = if input.large { 1440 } else { 320 };
    let cached = cache_path(catalog, &input.id, &path, bound)?;
    context.checkpoint()?;
    let bytes = if cached.is_file() {
        fs::read(&cached).map_err(|e| problem("cache_io", e))?
    } else {
        let image = decode_thumbnail(&path, bound)?;
        context.checkpoint()?;
        let bytes = encode_bounded(image, if input.large { 82 } else { 76 })?;
        context.checkpoint()?;
        publish_cache(&cached, &bytes)?;
        bytes
    };
    display_rendition(bytes, context)
}

fn resolve_original(catalog: &Catalog, id: &str) -> Result<PathBuf> {
    let (root, relative) = catalog.asset_location(id)?;
    let root = fs::canonicalize(root).map_err(|e| problem("folder_unavailable", e))?;
    let unresolved = root.join(relative);
    if is_link(&unresolved)? {
        return Err(Error::new(
            "image_unavailable",
            "Image was replaced by a link",
        ));
    }
    let path = fs::canonicalize(unresolved).map_err(|e| problem("image_unavailable", e))?;
    if !path.starts_with(&root) {
        return Err(Error::new(
            "image_unavailable",
            "Image is outside its registered folder",
        ));
    }
    Ok(path)
}

fn cache_path(catalog: &Catalog, id: &str, path: &Path, bound: u32) -> Result<PathBuf> {
    let metadata = fs::metadata(path).map_err(|e| problem("image_unavailable", e))?;
    let key = format!(
        "{id}-{}-{}-{bound}.jpg",
        metadata.len(),
        modified(&metadata)
    );
    let directory = catalog.directory().join("renditions");
    fs::create_dir_all(&directory).map_err(|e| problem("cache_io", e))?;
    Ok(directory.join(key))
}

fn decode_thumbnail(path: &Path, bound: u32) -> Result<image::RgbImage> {
    let mut reader = ImageReader::open(path)
        .map_err(|e| problem("image_decode", e))?
        .with_guessed_format()
        .map_err(|e| problem("image_decode", e))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| problem("image_decode", e))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| problem("image_decode", e))?;
    let mut decoded =
        image::DynamicImage::from_decoder(decoder).map_err(|e| problem("image_decode", e))?;
    decoded.apply_orientation(orientation);
    let rgba = decoded.thumbnail(bound, bound).to_rgba8();
    // Release the full-resolution allocation before compositing or encoding.
    drop(decoded);
    Ok(composite_on_white(&rgba))
}

fn composite_on_white(rgba: &image::RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y);
        let alpha = u32::from(pixel[3]);
        image::Rgb(
            [0, 1, 2].map(|i| ((u32::from(pixel[i]) * alpha + 255 * (255 - alpha)) / 255) as u8),
        )
    })
}

fn encode_bounded(mut image: image::RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(&image)
        .map_err(|e| problem("image_encode", e))?;
    let mut fallback = 1024;
    while bytes.len() > MAX_ENCODED_IMAGE && fallback >= 256 {
        image = image::imageops::thumbnail(&image, fallback, fallback);
        bytes.clear();
        JpegEncoder::new_with_quality(&mut bytes, 65)
            .encode_image(&image)
            .map_err(|e| problem("image_encode", e))?;
        fallback /= 2;
    }
    if bytes.len() > MAX_ENCODED_IMAGE {
        return Err(Error::new(
            "image_too_large",
            "Rendition exceeds its bounded byte budget",
        ));
    }
    Ok(bytes)
}

fn publish_cache(cached: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = cached.with_file_name(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, bytes).map_err(|e| problem("cache_io", e))?;
    // Another request may have already produced the identical rendition. Preserve
    // the original best-effort rename behavior and return our freshly encoded bytes.
    if fs::rename(&temporary, cached).is_err() {
        let _ = fs::remove_file(&temporary);
    }
    Ok(())
}

fn display_rendition(bytes: Vec<u8>, context: &TaskContext) -> Result<Rendition> {
    if bytes.len() > MAX_ENCODED_IMAGE {
        return Err(Error::new(
            "cache_invalid",
            "Cached rendition exceeds its budget",
        ));
    }
    let dimensions = image::load_from_memory(&bytes).map_err(|e| problem("cache_invalid", e))?;
    context.checkpoint()?;
    Ok(Rendition {
        data_url: format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)),
        width: dimensions.width(),
        height: dimensions.height(),
    })
}
