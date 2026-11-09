//! Operations on processes and the shell.

use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Ends a process, checking the creation time so a reused PID is never hit.
pub fn terminate(pid: u32, create_time: i64) -> Result<(), u32> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return Err(GetLastError());
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let same = GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) != 0
            && ((created.dwHighDateTime as i64) << 32 | created.dwLowDateTime as i64) == create_time;
        let result = if !same {
            Err(ERROR_INVALID_PARAMETER)
        } else if TerminateProcess(h, 1) == 0 {
            Err(GetLastError())
        } else {
            Ok(())
        };
        CloseHandle(h);
        result
    }
}

pub fn open_location(path: &str) {
    let args = wide(&format!("/select,\"{path}\""));
    let (verb, exe) = (wide("open"), wide("explorer.exe"));
    unsafe {
        ShellExecuteW(null_mut(), verb.as_ptr(), exe.as_ptr(), args.as_ptr(), null(), SW_SHOWNORMAL);
    }
}

pub fn properties(path: &str) {
    let (verb, file) = (wide("properties"), wide(path));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_INVOKEIDLIST;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.nShow = SW_SHOWNORMAL;
        ShellExecuteExW(&mut info);
    }
}

pub fn set_clipboard(text: &str) {
    const CF_UNICODETEXT: u32 = 13;
    let data = wide(text);
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
        if !mem.is_null() {
            let target = GlobalLock(mem) as *mut u16;
            std::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
            GlobalUnlock(mem);
            SetClipboardData(CF_UNICODETEXT, mem);
        }
        CloseClipboard();
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0;
        let ok = GetTokenInformation(token, TokenElevation, (&mut e as *mut TOKEN_ELEVATION).cast(), 4, &mut len);
        CloseHandle(token);
        ok != 0 && e.TokenIsElevated != 0
    }
}

/// Returns true if the elevated copy was started.
pub fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let (verb, file) = (wide("runas"), wide(&exe.to_string_lossy()));
    let result = unsafe {
        ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL)
    };
    result as isize > 32
}
