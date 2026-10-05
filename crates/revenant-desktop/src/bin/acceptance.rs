//! Headless native acceptance measurements for an existing, unchanged fixture.
//! Run: `cargo run -p revenant-desktop --no-default-features --locked --bin acceptance -- <fixturepath>`.

#[path = "../../tests/support/mod.rs"]
mod support;

#[tokio::main]
async fn main() {
    let Some(fixture) = std::env::args_os().nth(1) else {
        eprintln!("usage: acceptance <existing-fixture-directory>");
        std::process::exit(2);
    };
    let report = support::benchmark::run(fixture.into()).await;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
