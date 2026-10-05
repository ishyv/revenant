//! Process/contract fixture deliberately independent of the native SDK.
use std::{env, fs, net::TcpListener, process::Command, thread, time::Duration};
fn main() {
    let args: Vec<_> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--revenant-contract") => {
            println!("{}", fs::read_to_string("fixture-contract.json").unwrap());
            if std::path::Path::new("fixture-export-fails").exists() {
                eprintln!("intentional export failure");
                std::process::exit(19);
            }
        }
        Some("--descendant") => {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            fs::write(&args[2], listener.local_addr().unwrap().port().to_string()).unwrap();
            loop {
                thread::sleep(Duration::from_millis(100));
            }
        }
        Some("--spawn-descendant") | Some("--spawn-and-exit") => {
            let _child = Command::new(env::current_exe().unwrap())
                .args(["--descendant", &args[2]])
                .spawn()
                .unwrap();
            if args[1] == "--spawn-and-exit" {
                while !std::path::Path::new(&args[2]).is_file() {
                    thread::sleep(Duration::from_millis(10));
                }
                println!("descendant started");
                return;
            }
            loop {
                thread::sleep(Duration::from_millis(100));
            }
        }
        Some("--exit-failure") => {
            println!("stdout evidence");
            eprintln!("stderr evidence");
            std::process::exit(23);
        }
        _ => panic!("unexpected fixture invocation"),
    }
}
