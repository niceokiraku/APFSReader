//! One running copy of the app: a second launch brings the first to the front.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};

pub const WINDOW_TITLE: &str = "APFSReader";

/// True if this is the first copy. The mutex is held until the process exits.
pub fn acquire() -> bool {
    unsafe {
        let _handle = CreateMutexW(None, false, w!("Local\\APFSReader-GUI-single-instance"));
        // The handle is deliberately never closed: closing it would release the claim.
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// Show and focus the running copy's window, even if it is hidden in the tray.
pub fn activate_existing() {
    let title: Vec<u16> = WINDOW_TITLE.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}
