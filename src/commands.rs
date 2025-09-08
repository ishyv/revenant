use std::io;
use std::process::{ Command, Stdio };

/// Runs a shell command, returning its output or an error if something went wrong.
/// * @returns: Stdout as String on success, io::Error on failure.
pub fn run(line: &str) -> io::Result<String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", line]);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", line]);
        c
    };

    // Capture both stdout and stderr in one go.
    let out = cmd
        .stdin(Stdio::null())         // or inherit if you need input
        .stdout(Stdio::piped())       // CAPTURE stdout
        .stderr(Stdio::piped())       // CAPTURE stderr (optional)
        .output()?;                   // <-- run ONCE and collect

    if out.status.success() {
        // trim to drop the trailing newline most CLIs print
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        // include stderr in the error so you actually see why it failed
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "command failed: {}\nstderr: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ),
        ))
    }
}


#[macro_export]
/// * Utility macro to run shell commands, exiting on failure.
///  Usage: `cmd!( "command arg1 arg2", "another command" )`
macro_rules! cmd {
    // Single command
    ($raw:expr) => {
        match crate::commands::run($raw) {
            Ok(output) => {
                // If on debug show the output
                #[cfg(debug_assertions)]
                println!("Command '{}' output: {}", $raw, output);
                output

            } Err(e) => {
                eprintln!("Error running command '{}': {}", $raw, e);
                std::process::exit(1);
            }
        }
    };

    // Multiple commands
    (*($raw:expr),) => {
        {
            let output = Vec::new(); // To store command outputs if needed

            $(
                match crate::commands::run($raw) {
                    Ok(output) => {
                        // If on debug show the output
                        #[cfg(debug_assertions)]
                        println!("Command '{}' output: {}", $raw, output);

                        output.push(output);
                    }
                    Err(e) => {
                        eprintln!("Error running command '{}': {}", $raw, e);
                        std::process::exit(1);
                    }
                }
            )*

            output // Return all outputs as a Vec<String>
        }
    };
}

            