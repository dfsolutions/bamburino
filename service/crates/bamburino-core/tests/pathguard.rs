// SPDX-License-Identifier: AGPL-3.0-or-later
//! Path allowlist rules. Cases adapted from `BoundaryTests.cs` in
//! `Ding-Ding-Projects/BambuStudio` (AGPL-3.0, see `NOTICE.md`).
#![cfg(windows)]

use std::path::{Path, PathBuf};

use bamburino_core::pathguard::{PathError, PathGuard};

/// A temp tree: `<root>\Documents\a.stl`, `<root>\Documents\sub\b.stl`, and
/// `<root>\outside\secret.stl` (next to the allowed directory, not in it).
struct Tree {
    _dir: tempfile::TempDir,
    base: PathBuf,
    docs: PathBuf,
    guard: PathGuard,
}

fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    // Canonical spelling without the `\\?\` prefix.
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
    let docs = base.join("Documents");
    std::fs::create_dir_all(docs.join("sub")).unwrap();
    std::fs::create_dir_all(base.join("outside")).unwrap();
    std::fs::write(docs.join("a.stl"), b"x").unwrap();
    std::fs::write(docs.join("sub").join("b.stl"), b"x").unwrap();
    std::fs::write(base.join("outside").join("secret.stl"), b"x").unwrap();
    let (guard, excluded) = PathGuard::new(vec![docs.clone()]);
    assert!(excluded.is_empty(), "{excluded:?}");
    Tree {
        _dir: dir,
        base,
        docs,
        guard,
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn valid_paths_inside_the_root_are_accepted() {
    let t = tree();
    let d = s(&t.docs);
    for ok in [
        format!(r"{d}\a.stl"),
        format!(r"{d}\sub\b.stl"),
        format!(r"{d}\sub"),
        format!("{d}/sub/b.stl"),
        format!(r"{d}\.\a.stl"),
        format!(r"{d}\\a.stl"),
        format!(r"\\?\{d}\a.stl"),
    ] {
        let got = t.guard.check(&ok).unwrap_or_else(|e| panic!("{ok}: {e:?}"));
        assert!(got.exists(), "{ok}");
    }
}

#[test]
fn root_comparison_is_case_insensitive() {
    let t = tree();
    let lower = s(&t.docs.join("A.STL")).to_lowercase();
    assert!(t.guard.check(&lower).is_ok(), "{lower}");
    let upper = s(&t.docs.join("sub").join("b.stl")).to_uppercase();
    assert!(t.guard.check(&upper).is_ok(), "{upper}");
}

#[test]
fn negative_table() {
    let t = tree();
    let d = s(&t.docs);
    let base = s(&t.base);
    let cases: Vec<(String, PathError)> = vec![
        // Relative
        (r"..\..\Windows".into(), PathError::Relative),
        (r"a.stl".into(), PathError::Relative),
        (r"\Windows\win.ini".into(), PathError::Relative),
        (r"C:win.ini".into(), PathError::Relative),
        ("".into(), PathError::Relative),
        (format!("{d}\\a\u{7}.stl"), PathError::Relative),
        // Traversal
        (
            r"C:\Users\x\Documents\..\..\Windows\win.ini".into(),
            PathError::Traversal,
        ),
        (
            format!(r"{d}\sub\..\..\outside\secret.stl"),
            PathError::Traversal,
        ),
        // UNC
        (r"\\server\share\a.stl".into(), PathError::Unc),
        (r"\\?\UNC\server\share\a.stl".into(), PathError::Unc),
        ("//server/share/a.stl".into(), PathError::Unc),
        // Device paths
        (r"\\.\PhysicalDrive0".into(), PathError::DevicePath),
        (r"\\?\GLOBALROOT\Device\X".into(), PathError::DevicePath),
        (r"\\?\Volume{1234}\a.stl".into(), PathError::DevicePath),
        (r"\??\C:\a.stl".into(), PathError::DevicePath),
        // Reserved names
        (format!(r"{d}\CON.stl"), PathError::ReservedName),
        (format!(r"{d}\nul.txt"), PathError::ReservedName),
        (format!(r"{d}\COM1"), PathError::ReservedName),
        (format!(r"{d}\lpt9.x.y"), PathError::ReservedName),
        (format!("{d}\\com\u{b9}.stl"), PathError::ReservedName),
        (format!(r"{d}\a*.stl"), PathError::ReservedName),
        (format!(r"{d}\a?.stl"), PathError::ReservedName),
        // Alternate data streams
        (format!(r"{d}\a.stl:stream"), PathError::AlternateDataStream),
        (format!(r"{d}\a.stl::$DATA"), PathError::AlternateDataStream),
        // Trailing dot or space
        (format!(r"{d}\a.stl."), PathError::TrailingDotOrSpace),
        (format!(r"{d}\a.stl "), PathError::TrailingDotOrSpace),
        // Outside the roots
        (
            format!(r"{base}\outside\secret.stl"),
            PathError::OutsideRoots,
        ),
        (r"C:\Windows\win.ini".into(), PathError::OutsideRoots),
        (d.clone(), PathError::OutsideRoots), // the root itself is not "inside"
        (format!("{d}x\\a.stl"), PathError::OutsideRoots), // sibling with a common prefix
        // Missing
        (format!(r"{d}\missing.stl"), PathError::NotFound),
        (format!(r"{d}\a.stl\child"), PathError::NotFound), // file used as a directory
    ];
    for (input, want) in cases {
        assert_eq!(t.guard.check(&input), Err(want), "{input:?}");
    }
}

fn make_junction(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn junction_inside_root_pointing_outside_is_rejected() {
    let t = tree();
    let link = t.docs.join("link");
    if !make_junction(&link, &t.base.join("outside")) {
        eprintln!("SKIPPED: cannot create a junction here");
        return;
    }
    assert_eq!(
        t.guard.check(&s(&link.join("secret.stl"))),
        Err(PathError::ReparsePoint)
    );
    assert_eq!(t.guard.check(&s(&link)), Err(PathError::ReparsePoint));
    // Normal siblings still work.
    assert!(t.guard.check(&s(&t.docs.join("a.stl"))).is_ok());
}

#[test]
fn file_symlink_is_rejected() {
    let t = tree();
    let link = t.docs.join("sym.stl");
    if std::os::windows::fs::symlink_file(t.base.join("outside").join("secret.stl"), &link).is_err()
    {
        eprintln!("SKIPPED: symlink creation needs privilege or Developer Mode");
        return;
    }
    assert_eq!(t.guard.check(&s(&link)), Err(PathError::ReparsePoint));
}

#[test]
fn root_that_is_a_junction_is_excluded() {
    let t = tree();
    let link = t.base.join("rootlink");
    if !make_junction(&link, &t.docs) {
        eprintln!("SKIPPED: cannot create a junction here");
        return;
    }
    let (guard, excluded) = PathGuard::new(vec![link.clone()]);
    assert!(guard.roots().is_empty());
    assert_eq!(excluded, vec![(link, PathError::ReparsePoint)]);
}

#[test]
fn long_path_over_260_chars_is_accepted() {
    let t = tree();
    let mut deep = t.docs.clone();
    while deep.to_string_lossy().len() < 300 {
        deep.push("segment_with_a_long_name_0123456789");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let file = deep.join("long.stl");
    std::fs::write(&file, b"x").unwrap();
    assert!(s(&file).len() > 260);
    let got = t.guard.check(&s(&file)).unwrap();
    assert!(got.exists());
    // The `\\?\` long-path spelling is accepted too.
    assert!(t.guard.check(&format!(r"\\?\{}", s(&file))).is_ok());
}

#[test]
fn real_alternate_data_stream_is_rejected() {
    let t = tree();
    let file = t.docs.join("a.stl");
    // Create a real stream; ignore failure on file systems without ADS.
    let _ = std::fs::write(format!("{}:hidden", s(&file)), b"x");
    assert_eq!(
        t.guard.check(&format!("{}:hidden", s(&file))),
        Err(PathError::AlternateDataStream)
    );
}

#[test]
fn invalid_root_is_excluded_not_fatal() {
    let t = tree();
    let file_root = t.docs.join("a.stl");
    let roots = vec![
        t.docs.clone(),
        PathBuf::from(r"Z:\non\esiste"),
        PathBuf::from(r"relative\dir"),
        PathBuf::from(r"\\server\share"),
        file_root.clone(),
    ];
    let (guard, excluded) = PathGuard::new(roots);
    assert_eq!(guard.roots().len(), 1);
    assert_eq!(excluded.len(), 4);
    let errs: Vec<_> = excluded.iter().map(|(_, e)| *e).collect();
    assert_eq!(
        errs,
        vec![
            PathError::NotFound,
            PathError::Relative,
            PathError::Unc,
            PathError::NotFound
        ]
    );
    assert!(guard.check(&s(&t.docs.join("a.stl"))).is_ok());
}

#[test]
fn no_roots_rejects_everything() {
    let t = tree();
    let (guard, excluded) = PathGuard::new(vec![]);
    assert!(excluded.is_empty());
    assert_eq!(
        guard.check(&s(&t.docs.join("a.stl"))),
        Err(PathError::OutsideRoots)
    );
    // Syntax errors still win over "outside".
    assert_eq!(guard.check(r"\\server\share"), Err(PathError::Unc));
}

/// The user's Documents known folder (OneDrive-redirected on this PC).
fn documents_folder() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: out-pointer is valid; the returned string is freed below.
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut raw) };
    if hr != 0 || raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is a NUL-terminated UTF-16 string from the API.
    let text = unsafe {
        let mut len = 0;
        while *raw.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(raw, len))
    };
    // SAFETY: allocated by SHGetKnownFolderPath.
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(PathBuf::from(text))
}

#[test]
fn onedrive_documents_root_is_accepted() {
    let Some(docs) = documents_folder() else {
        eprintln!("SKIPPED: no Documents known folder");
        return;
    };
    if !docs.to_string_lossy().to_lowercase().contains("onedrive") {
        eprintln!("SKIPPED: Documents is not redirected to OneDrive");
        return;
    }
    let (guard, excluded) = PathGuard::new(vec![docs.clone()]);
    assert!(excluded.is_empty(), "{excluded:?}");
    // Any file directly inside (possibly a cloud placeholder) passes.
    let file = std::fs::read_dir(&docs)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false));
    match file {
        Some(f) => {
            let p = s(&f.path());
            guard.check(&p).unwrap_or_else(|e| panic!("{p}: {e:?}"));
        }
        None => eprintln!("no plain file in Documents: root acceptance only"),
    }
}
