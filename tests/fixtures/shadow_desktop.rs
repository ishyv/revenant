//! Tiny loaded-image fixture; descendants deliberately do not inherit log pipes.
use std::{
    env, fs,
    net::TcpListener,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
fn main() {
    if env::args().nth(1).as_deref() == Some("--descendant") {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        fs::write(
            "shadow.port",
            listener.local_addr().unwrap().port().to_string(),
        )
        .unwrap();
        loop {
            thread::sleep(Duration::from_millis(100));
        }
    }
    fs::write(
        "shadow.image",
        env::current_exe().unwrap().to_string_lossy().as_bytes(),
    )
    .unwrap();
    fs::write("shadow.generation", env!("REVENANT_SHADOW_GENERATION")).unwrap();
    let _child = Command::new(env::current_exe().unwrap())
        .arg("--descendant")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    loop {
        thread::sleep(Duration::from_millis(100));
    }
}
