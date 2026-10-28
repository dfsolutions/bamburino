// SPDX-License-Identifier: AGPL-3.0-or-later
//! Console echo control for reading a secret interactively.

/// Turns console echo off while alive; restores it on drop. `None` when stdin
/// is not an interactive console (redirected input needs no handling).
pub struct EchoOff {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    mode: u32,
}

#[cfg(windows)]
impl EchoOff {
    pub fn disable() -> Option<EchoOff> {
        use windows_sys::Win32::System::Console::{
            ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        // SAFETY: plain console API calls on the process's own stdin handle.
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) == 0 {
                return None; // redirected stdin
            }
            if SetConsoleMode(handle, mode & !ENABLE_ECHO_INPUT) == 0 {
                return None;
            }
            Some(EchoOff { handle, mode })
        }
    }
}

#[cfg(windows)]
impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: restores the mode read in `disable`.
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleMode(self.handle, self.mode);
        }
    }
}

#[cfg(not(windows))]
impl EchoOff {
    pub fn disable() -> Option<EchoOff> {
        None
    }
}
