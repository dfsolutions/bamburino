// SPDX-License-Identifier: AGPL-3.0-or-later
//! `bamburino.exe` entry point.

// Wired into the service in a later task.
#[allow(dead_code, unused_imports)]
mod process;

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version") => println!(
            "bamburino {}",
            bamburino_core::build_info::BAMBURINO_VERSION
        ),
        Some("config") => {
            let sub = std::env::args().nth(2);
            std::process::exit(run_config(sub.as_deref()));
        }
        _ => {
            eprintln!("usage: bamburino --version | config init | config set-driver-token");
            std::process::exit(2);
        }
    }
}

/// `config init` / `config set-driver-token` (token on STDIN, never echoed).
fn run_config(sub: Option<&str>) -> i32 {
    use bamburino::config::ConfigDir;
    use bamburino::secrets::{read_driver_token_from_stdin, write_secret_atomic};

    let dir = ConfigDir::resolve(std::env::var_os("BAMBURINO_CONFIG_DIR").map(Into::into));
    let result = match sub {
        Some("init") => dir
            .init()
            .map(|()| println!("configuration ready in {}", dir.0.display())),
        Some("set-driver-token") => dir.init().and_then(|()| {
            let token = read_driver_token_from_stdin(std::io::stdin().lock())?;
            write_secret_atomic(&dir.driver_token_file(), &token).map_err(|e| {
                bamburino::config::ConfigError::Io(dir.driver_token_file(), e.to_string())
            })?;
            println!("driver token saved");
            Ok(())
        }),
        _ => {
            eprintln!("usage: bamburino config init | config set-driver-token (token on stdin)");
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
