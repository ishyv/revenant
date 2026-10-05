//! Preservation tests exercise persisted catalog behavior and public wire contracts.
use super::*;
use crate::{contracts::RenderInput, rendition, scanning};
use revenant::TaskContext;

fn running() -> (TaskContext, revenant::TaskExecution) {
    let resources = revenant::ResourceRegistry::new();
    resources.create_scope("test", None).unwrap();
    let context = revenant::TaskManager::new(resources)
        .create("test", "work")
        .unwrap();
    let execution = context.begin().unwrap();
    (context, execution)
}

fn fixture() -> (tempfile::TempDir, Catalog, String) {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::open_at(directory.path()).unwrap();
    catalog
        .db
        .execute(
            "INSERT INTO roots(id,path,name) VALUES('root','/photos','Photos')",
            [],
        )
        .unwrap();
    catalog
        .insert_batch(
            "root",
            "first",
            &[ScannedImage {
                relative_path: "photo.jpg".into(),
                name: "photo.jpg".into(),
                size: "42".into(),
                modified: "1".into(),
            }],
        )
        .unwrap();
    let id = catalog
        .db
        .query_row("SELECT id FROM assets", [], |r| r.get::<_, String>(0))
        .unwrap();
    (directory, catalog, id)
}
fn request(search: &str) -> QueryInput {
    QueryInput {
        search: search.into(),
        root_id: String::new(),
        annotated_only: false,
        offset: 0,
        limit: 60,
    }
}
#[test]
fn notes_survive_reopen_refresh_and_accent_insensitive_search() {
    let (directory, mut catalog, id) = fixture();
    catalog
        .annotate(NoteInput {
            id: id.clone(),
            note: "Atardecer en Málaga".into(),
        })
        .unwrap();
    catalog
        .insert_batch(
            "root",
            "second",
            &[ScannedImage {
                relative_path: "photo.jpg".into(),
                name: "photo.jpg".into(),
                size: "64".into(),
                modified: "2".into(),
            }],
        )
        .unwrap();
    drop(catalog);
    let catalog = Catalog::open_at(directory.path()).unwrap();
    let result = catalog.query(request("malag atardec")).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items[0].id, id);
    assert_eq!(result.items[0].size, "64");
    assert_eq!(result.items[0].note, "Atardecer en Málaga");
}
#[test]
fn notes_update_search_and_pages_remain_bounded() {
    let (_directory, mut catalog, id) = fixture();
    let rows: Vec<_> = (0..140)
        .map(|i| ScannedImage {
            relative_path: format!("{i}.png"),
            name: format!("{i}.png"),
            size: "1".into(),
            modified: "1".into(),
        })
        .collect();
    catalog.insert_batch("root", "first", &rows).unwrap();
    let mut input = request("");
    input.limit = 1000;
    let page = catalog.query(input).unwrap();
    assert_eq!(page.total, 141);
    assert_eq!(page.items.len(), 60);
    catalog
        .annotate(NoteInput {
            id: id.clone(),
            note: "playa".into(),
        })
        .unwrap();
    assert_eq!(catalog.query(request("play")).unwrap().total, 1);
    catalog
        .annotate(NoteInput {
            id,
            note: "montaña".into(),
        })
        .unwrap();
    assert_eq!(catalog.query(request("play")).unwrap().total, 0);
    assert_eq!(catalog.query(request("montana")).unwrap().total, 1);
}
#[test]
fn literal_search_and_oversized_notes_are_handled() {
    let (_directory, catalog, id) = fixture();
    catalog.query(request("\" OR *")).unwrap();
    assert!(
        catalog
            .annotate(NoteInput {
                id,
                note: "x".repeat(MAX_NOTE_BYTES + 1)
            })
            .is_err()
    );
}

#[test]
fn real_folder_refresh_keeps_notes_and_renders_large_originals() {
    let directory = tempfile::tempdir().unwrap();
    let photos = directory.path().join("photos");
    fs::create_dir(&photos).unwrap();
    let mut seed = 42u32;
    let pixels = image::RgbImage::from_fn(1500, 1500, |_, _| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8])
    });
    let original = photos.join("large.png");
    pixels.save(&original).unwrap();
    assert!(fs::metadata(&original).unwrap().len() > 4 * 1024 * 1024);
    let mut catalog = Catalog::open_at(&directory.path().join("data")).unwrap();
    let (context, _execution) = running();
    let report = scanning::scan(&mut catalog, photos.to_str().unwrap(), &context).unwrap();
    assert_eq!(report.discovered, 1);
    let asset = catalog.query(request("")).unwrap().items.remove(0);
    catalog
        .annotate(NoteInput {
            id: asset.id.clone(),
            note: "Una fotografía grande".into(),
        })
        .unwrap();
    let rendition = rendition::render(
        &catalog,
        &RenderInput {
            id: asset.id.clone(),
            large: true,
        },
        &context,
    )
    .unwrap();
    assert!(rendition.data_url.len() < 1024 * 1024);
    assert!(rendition.width <= 1440 && rendition.height <= 1440);
    let again = rendition::render(
        &catalog,
        &RenderInput {
            id: asset.id.clone(),
            large: true,
        },
        &context,
    )
    .unwrap();
    assert_eq!(rendition.data_url, again.data_url);
    let (refresh, _refresh_execution) = running();
    scanning::scan(&mut catalog, photos.to_str().unwrap(), &refresh).unwrap();
    assert_eq!(
        catalog.query(request("fotografia")).unwrap().items[0].id,
        asset.id
    );
    fs::remove_file(&original).unwrap();
    let (refresh, _refresh_execution) = running();
    scanning::scan(&mut catalog, photos.to_str().unwrap(), &refresh).unwrap();
    assert_eq!(catalog.query(request("")).unwrap().total, 0);
    let note: String = catalog
        .db
        .query_row("SELECT note FROM assets WHERE id=?1", [asset.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(note, "Una fotografía grande");
}

#[test]
fn operation_ids_and_wire_shapes_are_preserved() {
    let manifest = crate::app().manifest().unwrap();
    let images: Vec<_> = manifest
        .operations
        .iter()
        .filter(|operation| operation.id.starts_with("images."))
        .collect();
    assert_eq!(
        images
            .iter()
            .map(|operation| operation.id.as_str())
            .collect::<Vec<_>>(),
        [
            "images.annotate",
            "images.bootstrap",
            "images.query",
            "images.removeRoot",
            "images.render",
            "images.scan"
        ]
    );
    assert_eq!(
        revenant::serde_json::to_value(QueryInput {
            search: "sunset".into(),
            root_id: "folder".into(),
            annotated_only: true,
            offset: 2,
            limit: 60,
        })
        .unwrap(),
        revenant::serde_json::json!({
            "search": "sunset", "rootId": "folder", "annotatedOnly": true, "offset": 2, "limit": 60
        })
    );
}

fn image_fixture(pixels: &image::DynamicImage) -> (tempfile::TempDir, Catalog, Asset, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let photos = directory.path().join("photos");
    fs::create_dir(&photos).unwrap();
    let original = photos.join("photo.png");
    pixels.save(&original).unwrap();
    let mut catalog = Catalog::open_at(&directory.path().join("data")).unwrap();
    let (context, _execution) = running();
    scanning::scan(&mut catalog, photos.to_str().unwrap(), &context).unwrap();
    let asset = catalog.query(request("")).unwrap().items.remove(0);
    (directory, catalog, asset, original)
}

#[test]
fn root_removal_persists_cascades_notes_and_leaves_original_files() {
    let (directory, catalog, asset, original) = image_fixture(&image::DynamicImage::new_rgb8(1, 1));
    catalog
        .annotate(NoteInput {
            id: asset.id.clone(),
            note: "Retire this note".into(),
        })
        .unwrap();
    assert!(
        catalog
            .remove_root(RootInput {
                id: asset.root_id.clone()
            })
            .unwrap()
            .removed
    );
    assert!(original.is_file());
    assert!(
        !catalog
            .remove_root(RootInput { id: asset.root_id })
            .unwrap()
            .removed
    );
    drop(catalog);
    let catalog = Catalog::open_at(&directory.path().join("data")).unwrap();
    assert!(catalog.bootstrap().unwrap().roots.is_empty());
    assert_eq!(catalog.query(request("retire")).unwrap().total, 0);
    assert_eq!(
        catalog
            .annotate(NoteInput {
                id: asset.id,
                note: "Missing".into()
            })
            .err()
            .unwrap()
            .code,
        "asset_missing"
    );
}

#[test]
fn thumbnails_composite_transparency_on_white_and_follow_original_changes() {
    use base64::Engine;
    let transparent = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        640,
        320,
        image::Rgba([12, 34, 56, 0]),
    ));
    let (_directory, catalog, asset, original) = image_fixture(&transparent);
    let input = RenderInput {
        id: asset.id,
        large: false,
    };
    let (context, _execution) = running();
    let preview = rendition::render(&catalog, &input, &context).unwrap();
    assert_eq!((preview.width, preview.height), (320, 160));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            preview
                .data_url
                .strip_prefix("data:image/jpeg;base64,")
                .unwrap(),
        )
        .unwrap();
    let pixels = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert_eq!(pixels.get_pixel(0, 0).0, [255, 255, 255]);
    fs::write(&original, b"not an image any more").unwrap();
    assert_eq!(
        rendition::render(&catalog, &input, &context)
            .err()
            .unwrap()
            .code,
        "image_decode"
    );
    image::RgbImage::new(6, 6).save(&original).unwrap();
    let preview = rendition::render(&catalog, &input, &context).unwrap();
    // The existing thumbnail API scales small originals up to the requested bound.
    assert_eq!((preview.width, preview.height), (320, 320));
    fs::remove_file(original).unwrap();
    assert_eq!(
        rendition::render(&catalog, &input, &context)
            .err()
            .unwrap()
            .code,
        "image_unavailable"
    );
}

#[test]
fn invalid_and_oversized_cached_renditions_keep_the_cache_error_code() {
    let (_directory, catalog, asset, _original) =
        image_fixture(&image::DynamicImage::new_rgb8(1, 1));
    let input = RenderInput {
        id: asset.id,
        large: false,
    };
    let (context, _execution) = running();
    rendition::render(&catalog, &input, &context).unwrap();
    let cached = fs::read_dir(catalog.directory().join("renditions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(&cached, b"invalid JPEG").unwrap();
    assert_eq!(
        rendition::render(&catalog, &input, &context)
            .err()
            .unwrap()
            .code,
        "cache_invalid"
    );
    fs::write(&cached, vec![0; 700 * 1024 + 1]).unwrap();
    assert_eq!(
        rendition::render(&catalog, &input, &context)
            .err()
            .unwrap()
            .code,
        "cache_invalid"
    );
}

#[test]
fn images_over_the_decoder_axis_limit_are_rejected() {
    let (_directory, catalog, asset, _original) =
        image_fixture(&image::DynamicImage::new_rgb8(20_001, 1));
    let (context, _execution) = running();
    assert_eq!(
        rendition::render(
            &catalog,
            &RenderInput {
                id: asset.id,
                large: true
            },
            &context
        )
        .err()
        .unwrap()
        .code,
        "image_decode"
    );
}

#[test]
fn rendition_resolution_rejects_originals_outside_the_registered_root() {
    let (directory, mut catalog, asset, _original) =
        image_fixture(&image::DynamicImage::new_rgb8(1, 1));
    let outside = directory.path().join("outside.png");
    image::RgbImage::new(1, 1).save(&outside).unwrap();
    catalog
        .insert_batch(
            &asset.root_id,
            "outside",
            &[ScannedImage {
                relative_path: outside.to_string_lossy().into_owned(),
                name: "outside.png".into(),
                size: "1".into(),
                modified: "1".into(),
            }],
        )
        .unwrap();
    let outside_asset = catalog.query(request("outside")).unwrap().items.remove(0);
    let (context, _execution) = running();
    let error = rendition::render(
        &catalog,
        &RenderInput {
            id: outside_asset.id,
            large: false,
        },
        &context,
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "image_unavailable");
    assert_eq!(error.message, "Image is outside its registered folder");
}

#[test]
fn cancelled_refresh_preserves_missing_images_and_annotations() {
    let directory = tempfile::tempdir().unwrap();
    let photos = directory.path().join("photos");
    fs::create_dir(&photos).unwrap();
    let original = photos.join("photo.png");
    image::RgbImage::new(1, 1).save(&original).unwrap();
    let mut catalog = Catalog::open_at(&directory.path().join("data")).unwrap();
    let (context, _execution) = running();
    scanning::scan(&mut catalog, photos.to_str().unwrap(), &context).unwrap();
    let asset = catalog.query(request("")).unwrap().items.remove(0);
    catalog
        .annotate(NoteInput {
            id: asset.id.clone(),
            note: "Keep this".into(),
        })
        .unwrap();
    fs::remove_file(&original).unwrap();
    let (refresh, _refresh_execution) = running();
    refresh.request_cancel().unwrap();
    assert_eq!(
        scanning::scan(&mut catalog, photos.to_str().unwrap(), &refresh)
            .err()
            .unwrap()
            .code,
        "cancelled"
    );
    let result = catalog.query(request("keep")).unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items[0].id, asset.id);
    assert_eq!(result.items[0].note, "Keep this");
}

#[test]
fn scanning_skips_its_data_directory_and_reconciles_multiple_batches() {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::open_at(&directory.path().join("data")).unwrap();
    image::RgbImage::new(1, 1)
        .save(directory.path().join("data/hidden.png"))
        .unwrap();
    for i in 0..130 {
        // Scanning uses metadata only, so contents need not decode as an image.
        fs::write(
            directory.path().join(format!("{i:03}.PNG")),
            b"metadata only",
        )
        .unwrap();
    }
    fs::write(directory.path().join("ignored.txt"), b"not an image").unwrap();
    let (context, _execution) = running();
    let report =
        scanning::scan(&mut catalog, directory.path().to_str().unwrap(), &context).unwrap();
    assert_eq!((report.discovered, report.warnings), (130, 0));
    assert_eq!(catalog.query(request("")).unwrap().total, 130);
    assert_eq!(context.snapshot().progress.completed.get(), 130);
    fs::remove_file(directory.path().join("000.PNG")).unwrap();
    let (context, _execution) = running();
    let report =
        scanning::scan(&mut catalog, directory.path().to_str().unwrap(), &context).unwrap();
    assert_eq!(report.root_id, catalog.bootstrap().unwrap().roots[0].id);
    assert_eq!(report.discovered, 129);
    assert_eq!(catalog.bootstrap().unwrap().roots[0].count, 129);
}

#[test]
fn note_and_query_limits_measure_utf8_bytes() {
    let (_directory, catalog, id) = fixture();
    let note = "é".repeat(8 * 1024);
    catalog
        .annotate(NoteInput {
            id: id.clone(),
            note: note.clone(),
        })
        .unwrap();
    assert_eq!(
        catalog
            .annotate(NoteInput {
                id,
                note: format!("{note}x")
            })
            .err()
            .unwrap()
            .code,
        "note_too_large"
    );
    catalog.query(request(&"é".repeat(512))).unwrap();
    assert_eq!(
        catalog.query(request(&"é".repeat(513))).err().unwrap().code,
        "query_too_long"
    );
    assert_eq!(
        catalog
            .annotate(NoteInput {
                id: "absent".into(),
                note: "hello".into()
            })
            .err()
            .unwrap()
            .code,
        "asset_missing"
    );
}
