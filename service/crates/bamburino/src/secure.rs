// SPDX-License-Identifier: AGPL-3.0-or-later
//! Race-free creation of protected objects (Windows API): files and the
//! configuration root are created with their final security descriptor, so
//! there is no window in which a weaker DACL applies.

use std::io;
use std::path::Path;

use crate::acl::{OwnerAccess, valid_sid};

/// SDDL of a secret file: protected, SYSTEM/Administrators full, LOCAL SERVICE
/// read, the installing user read or modify.
pub(crate) fn file_sddl(owner_sid: &str, owner: OwnerAccess) -> String {
    let owner_mask = match owner {
        OwnerAccess::Read => "FR",
        OwnerAccess::Modify => "0x1301bf",
    };
    format!("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;LS)(A;;{owner_mask};;;{owner_sid})")
}

/// SDDL of the configuration root: owned by Administrators, protected,
/// SYSTEM/Administrators full, LOCAL SERVICE and the installing user read and
/// execute only (nobody but an administrator can create entries in it).
pub fn root_sddl(owner_sid: &str) -> String {
    format!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;LS)(A;OICI;FRFX;;;{owner_sid})")
}

/// Canonical form of an SDDL trustee: well-known SIDs map to their two-letter
/// alias, as Windows prints them, and the RID-500/501 accounts of a domain
/// SID (`S-1-5-21-a-b-c-500`) map to `LA`/`LG` (the built-in Administrator
/// and Guest). Anything else is returned unchanged, so unknown trustees never
/// compare equal to a well-known one (fail closed).
pub fn canonical_trustee(t: &str) -> String {
    match t {
        "S-1-5-18" => return "SY".to_owned(),
        "S-1-5-32-544" => return "BA".to_owned(),
        "S-1-5-19" => return "LS".to_owned(),
        "S-1-5-32-545" => return "BU".to_owned(),
        "S-1-1-0" => return "WD".to_owned(),
        "S-1-5-11" => return "AU".to_owned(),
        "S-1-5-4" => return "IU".to_owned(),
        _ => {}
    }
    if let Some(rest) = t.strip_prefix("S-1-5-21-") {
        let parts: Vec<&str> = rest.split('-').collect();
        let numeric = parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        if numeric && parts.len() == 4 {
            match parts[3] {
                "500" => return "LA".to_owned(),
                "501" => return "LG".to_owned(),
                _ => {}
            }
        }
    }
    t.to_owned()
}

fn check_sid(sid: &str) -> io::Result<()> {
    if valid_sid(sid) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "owner_sid is not a valid SID",
        ))
    }
}

/// Creates a new secret file (exclusive share mode, protected DACL from the
/// first instant). Any stale file at `path` is removed first.
#[cfg(windows)]
pub fn create_restricted_file(
    path: &Path,
    owner_sid: &str,
    owner: OwnerAccess,
) -> io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};

    check_sid(owner_sid)?;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let sd = win::SecurityDescriptor::from_sddl(&file_sddl(owner_sid, owner))?;
    let wide = win::wide(path);
    // SAFETY: `wide` is NUL-terminated and `sd.attributes()` points to a live
    // descriptor owned by `sd` for the duration of the call.
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0, // exclusive: nobody else can open it while we hold it
            sd.attributes(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `h` is a valid handle we own.
    Ok(unsafe { std::fs::File::from_raw_handle(h as _) })
}

#[cfg(not(windows))]
pub fn create_restricted_file(
    path: &Path,
    owner_sid: &str,
    _owner: OwnerAccess,
) -> io::Result<std::fs::File> {
    check_sid(owner_sid)?;
    let _ = std::fs::remove_file(path);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Creates the configuration root with its final protected DACL and
/// Administrators as owner. Needs an elevated prompt.
#[cfg(windows)]
pub fn create_protected_root(path: &Path, owner_sid: &str) -> io::Result<()> {
    check_sid(owner_sid)?;
    create_dir_with_sddl(path, &root_sddl(owner_sid))
}

/// Creates a directory with the given SDDL as its security descriptor.
/// Exposed so tests can exercise the DACL without an elevated token.
#[doc(hidden)]
#[cfg(windows)]
pub fn create_dir_with_sddl(path: &Path, sddl: &str) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    let sd = win::SecurityDescriptor::from_sddl(sddl)?;
    let wide = win::wide(path);
    // SAFETY: as in `create_restricted_file`.
    if unsafe { CreateDirectoryW(wide.as_ptr(), sd.attributes()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn create_protected_root(path: &Path, owner_sid: &str) -> io::Result<()> {
    check_sid(owner_sid)?;
    std::fs::create_dir(path)
}

/// SDDL (owner + DACL) of an existing object.
#[cfg(windows)]
pub fn security_sddl(path: &Path) -> io::Result<String> {
    win::sddl_of(path)
}

/// Why an existing configuration root cannot be trusted, if it cannot: the
/// owner must be SYSTEM or Administrators and the DACL protected.
pub fn root_problem(path: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return Some("it is a reparse point (symlink or junction)".to_owned());
            }
            Ok(_) => {}
            Err(e) => return Some(format!("cannot inspect it: {e}")),
        }
        match security_sddl(path) {
            Err(e) => Some(format!("cannot read its security descriptor: {e}")),
            Ok(sddl) => {
                if !(sddl.starts_with("O:BA") || sddl.starts_with("O:SY")) {
                    Some("its owner is not SYSTEM or Administrators".to_owned())
                } else if !sddl.contains("D:P") {
                    Some("its DACL is not protected (it inherits permissions)".to_owned())
                } else {
                    None
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SECURITY_ATTRIBUTES,
    };

    pub fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// An allocated security descriptor plus the attributes pointing at it.
    pub struct SecurityDescriptor {
        sd: PSECURITY_DESCRIPTOR,
        attrs: SECURITY_ATTRIBUTES,
    }

    impl SecurityDescriptor {
        pub fn from_sddl(sddl: &str) -> io::Result<Self> {
            let w: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: `w` is NUL-terminated; `sd` receives a LocalAlloc'd
            // descriptor freed in `Drop`.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    w.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                sd,
                attrs: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: sd,
                    bInheritHandle: 0,
                },
            })
        }

        pub fn attributes(&self) -> *const SECURITY_ATTRIBUTES {
            &self.attrs
        }
    }

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: allocated by the conversion call in `from_sddl`.
            unsafe { LocalFree(self.sd) };
        }
    }

    pub fn sddl_of(path: &Path) -> io::Result<String> {
        let w = wide(path);
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: out-pointers are valid; the owner/DACL pointers are unused.
        let rc = unsafe {
            GetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let mut s: *mut u16 = std::ptr::null_mut();
        // SAFETY: `sd` stays valid until freed below.
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut s,
                std::ptr::null_mut(),
            )
        };
        let result = if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: `s` is a NUL-terminated UTF-16 string from the API.
            let text = unsafe {
                let mut len = 0;
                while *s.add(len) != 0 {
                    len += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(s, len))
            };
            // SAFETY: allocated by the conversion call.
            unsafe { LocalFree(s.cast()) };
            Ok(text)
        };
        // SAFETY: allocated by GetNamedSecurityInfoW.
        unsafe { LocalFree(sd) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_trustee as c;

    #[test]
    fn well_known_sids_map_to_aliases() {
        for (sid, alias) in [
            ("S-1-5-18", "SY"),
            ("S-1-5-32-544", "BA"),
            ("S-1-5-19", "LS"),
            ("S-1-5-32-545", "BU"),
            ("S-1-1-0", "WD"),
            ("S-1-5-11", "AU"),
            ("S-1-5-4", "IU"),
        ] {
            assert_eq!(c(sid), alias);
            assert_eq!(c(alias), alias);
        }
    }

    #[test]
    fn builtin_administrator_and_guest_map_to_la_and_lg() {
        assert_eq!(c("S-1-5-21-3776380004-1005456608-3524014338-500"), "LA");
        assert_eq!(c("S-1-5-21-3776380004-1005456608-3524014338-501"), "LG");
        assert_eq!(c("LA"), "LA");
    }

    #[test]
    fn other_trustees_are_unchanged() {
        for s in [
            "S-1-5-21-1-2-3-1001",
            "S-1-5-21-1-2-3-5000",
            "S-1-5-21-1-2-500",
            "S-1-5-21-1-2-3-4-500",
            "S-1-5-21-a-2-3-500",
            "S-1-5-32-500",
            "",
        ] {
            assert_eq!(c(s), s);
        }
    }
}
