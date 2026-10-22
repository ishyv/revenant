//! Headless native acceptance measurements for an existing, unchanged fixture.
//! Run: `cargo run -p revenant-desktop --no-default-features --locked --bin acceptance -- <fixturepath> [--output <reportpath>]`.
//! Use `--efficiency` in place of the fixture path to compare scheduling policies.

#[path = "../../tests/support/mod.rs"]
mod support;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let Some(fixture) = arguments.next() else {
        eprintln!(
            "usage: acceptance <existing-fixture-directory>|--efficiency [--output <reportpath>]"
        );
        std::process::exit(2);
    };
    let output = match arguments.next() {
        None => None,
        Some(flag) if flag == "--output" => {
            Some(arguments.next().expect("--output requires a report path"))
        }
        Some(_) => panic!("unknown acceptance argument"),
    };
    assert!(arguments.next().is_none(), "unexpected acceptance argument");
    let report = if fixture == "--efficiency" {
        support::benchmark::compare(support::benchmark::ComparisonOptions::default()).await
    } else {
        support::benchmark::run(fixture.into()).await
    };
    let json = serde_json::to_string_pretty(&report).unwrap();
    if let Some(path) = output {
        std::fs::write(&path, format!("{json}\n")).expect("write acceptance report");
        eprintln!(
            "Acceptance report saved to {}",
            std::path::Path::new(&path).display()
        );
    } else {
        println!("{json}");
    }
}
