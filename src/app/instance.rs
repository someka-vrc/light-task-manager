//! One running copy of the app per user.
//!
//! A named mutex marks the running copy. A second start brings the first one's window
//! forward and ends. When the running copy restarts itself as administrator, the new
//! copy is started with `--after-elevate` and waits a few seconds for the old one to go.

use crate::system::win::wide;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// The command-line flag of a copy started by "restart as administrator".
pub const AFTER_ELEVATE: &str = "--after-elevate";
/// The window class of the main window.
pub const WINDOW_CLASS: &str = "LightTaskManagerWindow";

const MUTEX_NAME: &str = "Local\\LightTaskManager.SingleInstance";
/// How long a copy that replaces another waits for it to end.
const HANDOVER_MS: u32 = 5000;

/// Held for as long as the app runs.
pub struct Guard(HANDLE);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

/// The mutex's name. A debug build can take another one, to run beside a running copy:
///  in the environment.
fn mutex_name() -> String {
    #[cfg(debug_assertions)]
    if let Ok(suffix) = std::env::var("LTM_INSTANCE") {
        return format!("{MUTEX_NAME}.{suffix}");
    }
    MUTEX_NAME.to_string()
}

/// Takes the right to run. Returns None when another copy is running; its window is then
/// brought forward.
pub fn acquire() -> Option<Guard> {
    let after_elevate = std::env::args().any(|a| a == AFTER_ELEVATE);
    let name = wide(&mutex_name());
    let h = unsafe { CreateMutexW(null(), 1, name.as_ptr()) };
    let err = unsafe { GetLastError() };
    if h.is_null() {
        // A copy running with more rights owns the mutex and does not let us open it.
        if err == ERROR_ACCESS_DENIED {
            bring_forward();
            return None;
        }
        // The mutex could not be made for another reason: run rather than refuse to start.
        return Some(Guard(null_mut()));
    }
    if err != ERROR_ALREADY_EXISTS {
        return Some(Guard(h));
    }
    if after_elevate {
        // The copy that started us is closing; the mutex becomes ours when it is gone.
        let wait = unsafe { WaitForSingleObject(h, HANDOVER_MS) };
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Some(Guard(h));
        }
    }
    unsafe { CloseHandle(h) };
    bring_forward();
    None
}

fn bring_forward() {
    let class = wide(WINDOW_CLASS);
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), null());
        if hwnd.is_null() {
            return;
        }
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
}
