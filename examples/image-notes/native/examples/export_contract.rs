//! Verify the compiled application contract without creating a desktop window.
fn main() -> revenant::Result<()> {
    let manifest = image_notes_native::app().manifest()?;
    println!("{}", revenant::serde_json::to_string(&manifest)?);
    Ok(())
}
