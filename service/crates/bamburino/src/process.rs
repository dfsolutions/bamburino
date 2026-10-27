// SPDX-License-Identifier: AGPL-3.0-or-later
//! Windows process probe: is `bambu-studio.exe` running?

#[cfg(windows)]
pub use imp::WindowsProcessProbe;

#[cfg(windows)]
mod imp {
    use bamburino_core::detect::ProcessProbe;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    const STUDIO_EXE: &str = "bambu-studio.exe";

    #[derive(Debug, Default, Clone, Copy)]
    pub struct WindowsProcessProbe;

    /// Case-insensitive comparison of a NUL-terminated UTF-16 name.
    fn is_studio(exe_file: &[u16]) -> bool {
        let len = exe_file
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(exe_file.len());
        String::from_utf16_lossy(&exe_file[..len]).eq_ignore_ascii_case(STUDIO_EXE)
    }

    impl ProcessProbe for WindowsProcessProbe {
        fn studio_running(&self) -> Option<bool> {
            // SAFETY: plain Win32 snapshot enumeration; `entry` is a properly
            // sized, zero-initialised PROCESSENTRY32W and the handle is closed
            // on every path.
            unsafe {
                let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
                if snap == INVALID_HANDLE_VALUE {
                    return None;
                }
                let mut entry: PROCESSENTRY32W = std::mem::zeroed();
                entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                let mut found = false;
                let mut ok = Process32FirstW(snap, &mut entry) != 0;
                while ok {
                    if is_studio(&entry.szExeFile) {
                        found = true;
                        break;
                    }
                    ok = Process32NextW(snap, &mut entry) != 0;
                }
                CloseHandle(snap);
                Some(found)
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn process_probe_does_not_panic() {
            let _ = WindowsProcessProbe.studio_running();
        }

        #[test]
        fn exe_name_match_is_case_insensitive() {
            let w = |s: &str| {
                let mut v: Vec<u16> = s.encode_utf16().collect();
                v.push(0);
                v.push(b'x' as u16);
                v
            };
            assert!(is_studio(&w("Bambu-Studio.EXE")));
            assert!(!is_studio(&w("bambu-studio.exe.bak")));
        }
    }
}
