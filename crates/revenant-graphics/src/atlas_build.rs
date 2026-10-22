//! Deterministic fixed-font rasterization, performed during packaging/build.
use sha2::{Digest, Sha256};

pub(crate) fn prepare() -> std::io::Result<()> {
    let bytes = include_bytes!("../assets/AtkinsonHyperlegible-Regular.ttf");
    let font = fontdue::Font::from_bytes(bytes as &[u8], Default::default())
        .map_err(std::io::Error::other)?;
    let mut pixels = vec![0; 1024 * 1024];
    let mut glyphs = Vec::with_capacity(352 * 36);
    let (mut x, mut y, mut row) = (1usize, 1usize, 0usize);
    for ch in (32..384).filter_map(char::from_u32) {
        let (m, bitmap) = font.rasterize(ch, 18.);
        if x + m.width + 1 >= 1024 {
            x = 1;
            y += row + 2;
            row = 0;
        }
        if y + m.height + 1 >= 1024 {
            return Err(std::io::Error::other("Native font atlas overflow"));
        }
        for r in 0..m.height {
            let dst = (y + r) * 1024 + x;
            pixels[dst..dst + m.width].copy_from_slice(&bitmap[r * m.width..(r + 1) * m.width]);
        }
        glyphs.extend_from_slice(&(ch as u32).to_le_bytes());
        for v in [
            x as f32 / 1024.,
            y as f32 / 1024.,
            m.width as f32 / 1024.,
            m.height as f32 / 1024.,
            m.width as f32,
            m.height as f32,
            m.advance_width,
            m.ymin as f32,
        ] {
            glyphs.extend_from_slice(&v.to_le_bytes());
        }
        x += m.width + 2;
        row = row.max(m.height);
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR missing"));
    std::fs::write(out.join("scene-atlas.bin"), pixels)?;
    std::fs::write(out.join("scene-glyphs.bin"), glyphs)?;
    std::fs::write(
        out.join("scene-font.rs"),
        format!(
            "/// SHA-256 of the fixed OFL font, rasterized at build time for glyphs 32..383.\npub const FONT_SHA256: &str = \"{:x}\";\n",
            Sha256::digest(bytes)
        ),
    )?;
    Ok(())
}
