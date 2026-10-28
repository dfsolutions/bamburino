// SPDX-License-Identifier: AGPL-3.0-or-later
//! Excluded TCP port range parsing and the port preflight.

use bamburino::config::Config;
use bamburino::ports::{Ranges, is_excluded, parse_excluded_ranges, preflight, reserved_message};

/// Captured `netsh interface ipv4 show excludedportrange protocol=tcp`.
const NETSH: &str = "
Protocol tcp Port Exclusion Ranges

Start Port    End Port
----------    --------
      1301        1400
      2869        2869
     27021       27120
     27121       27220
     50000       50059     *

* - Administered port exclusions.
";

fn ranges() -> Ranges {
    parse_excluded_ranges(NETSH)
}

#[test]
fn parses_the_netsh_table() {
    assert_eq!(
        ranges(),
        [
            (1301, 1400),
            (2869, 2869),
            (27021, 27120),
            (27121, 27220),
            (50000, 50059)
        ]
    );
    assert!(parse_excluded_ranges("").is_empty());
    assert!(parse_excluded_ranges("garbage 1 2\nStart Port End Port").is_empty());
}

#[test]
fn range_membership_is_inclusive() {
    let r = ranges();
    assert!(is_excluded(27121, &r));
    assert!(is_excluded(27183, &r));
    assert!(is_excluded(27220, &r));
    assert!(!is_excluded(27221, &r));
    assert!(!is_excluded(27600, &r));
    assert!(!is_excluded(27601, &r));
}

#[test]
fn refuses_an_excluded_listen_port() {
    let cfg = Config {
        listen_port: 27190,
        ..Config::default()
    };
    let p = preflight(&cfg, &|| Ok(ranges()));
    let msg = p.refusal.expect("must refuse");
    assert_eq!(msg, reserved_message(27190));
    assert!(msg.contains("excluded port range"));
    assert!(msg.contains("net stop winnat && net start winnat"));
    assert!(msg.contains("listen_port"));
}

#[test]
fn default_config_passes_and_warns_about_the_excluded_driver_port() {
    let p = preflight(&Config::default(), &|| Ok(ranges()));
    assert_eq!(p.refusal, None);
    assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
    assert!(p.warnings[0].contains("27183"));
    assert!(p.warnings[0].contains("27601"));
}

#[test]
fn no_exclusions_means_no_message() {
    let p = preflight(&Config::default(), &|| Ok(Vec::new()));
    assert_eq!(p, Default::default());
}

#[test]
fn unreadable_ranges_only_warn() {
    let p = preflight(&Config::default(), &|| Err(std::io::Error::other("boom")));
    assert_eq!(p.refusal, None);
    assert_eq!(p.warnings.len(), 1);
}
