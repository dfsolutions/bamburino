// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build-time constants taken from `driver/upstream.toml`.

include!(concat!(env!("OUT_DIR"), "/build_info_generated.rs"));

/// Version of this service (from the workspace package version).
pub const BAMBURINO_VERSION: &str = env!("CARGO_PKG_VERSION");
