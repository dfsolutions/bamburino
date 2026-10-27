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
        _ => {
            eprintln!("usage: bamburino --version");
            std::process::exit(2);
        }
    }
}
