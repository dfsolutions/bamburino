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

/// `config init` (elevated) / `config set-driver-token` (token on STDIN,
/// never echoed, never an argument).
///
/// `set-driver-token` does NOT need elevation and does not call `init`: it
/// overwrites the existing `driver.token` in place, which the installing
/// user may modify. It fails loudly if `config init` was never run.
fn run_config(sub: Option<&str>) -> i32 {
    use bamburino::config::{ConfigDir, ConfigError};
    use bamburino::console::EchoOff;
    use bamburino::secrets::{overwrite_secret, read_driver_token_from_stdin};

    let dir = ConfigDir::resolve(std::env::var_os("BAMBURINO_CONFIG_DIR").map(Into::into));
    let result = match sub {
        Some("init") => dir
            .init()
            .map(|()| println!("configuration ready in {}", dir.0.display())),
        Some("set-driver-token") => (|| {
            let path = dir.driver_token_file();
            if !path.exists() {
                return Err(ConfigError::Invalid(format!(
                    "{} not found: run `bamburino config init` from an elevated prompt first",
                    path.display()
                )));
            }
            let echo = EchoOff::disable();
            if echo.is_some() {
                eprint!("Driver token (64 hex characters, input hidden): ");
            }
            let token = read_driver_token_from_stdin(std::io::stdin().lock());
            if echo.is_some() {
                eprintln!();
            }
            drop(echo);
            overwrite_secret(&path, &token?)
                .map_err(|e| ConfigError::Io(path.clone(), e.to_string()))?;
            println!("driver token saved");
            Ok(())
        })(),
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
