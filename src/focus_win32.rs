//! Focus a Windows Terminal / VS Code / cmd window by title-match, and
//! synthesize keyboard input via SendInput with a foreground-guard.

use anyhow::{anyhow, Result};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::thread::sleep;
use std::time::Duration;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowTextW, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

pub const CLASS_WT: &str = "CASCADIA_HOSTING_WINDOW_CLASS";
pub const CLASS_VSCODE: &str = "Chrome_WidgetWin_1"; // VS Code / Electron

fn hwnd_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    OsString::from_wide(&buf[..len as usize]).to_string_lossy().into_owned()
}

fn hwnd_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    OsString::from_wide(&buf[..len as usize]).to_string_lossy().into_owned()
}

struct EnumState {
    class_filter: Option<&'static str>,
    needle: String,
    best: Option<HWND>,
}

extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = unsafe { &mut *(lparam.0 as *mut EnumState) };
    if unsafe { !IsWindowVisible(hwnd).as_bool() } { return true.into(); }
    let class = hwnd_class(hwnd);
    if let Some(cf) = state.class_filter {
        if !class.eq_ignore_ascii_case(cf) { return true.into(); }
    }
    let title = hwnd_title(hwnd).to_lowercase();
    if title.contains(&state.needle) {
        state.best = Some(hwnd);
        return false.into();
    }
    true.into()
}

pub fn find_window_by_title(class_filter: Option<&'static str>, needle: &str) -> Option<HWND> {
    let mut state = EnumState { class_filter, needle: needle.to_lowercase(), best: None };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut state as *mut _ as isize));
    }
    state.best
}

/// Returns (class, title) of the current foreground window, or ("","") if none.
pub fn foreground_info() -> (String, String) {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 { return (String::new(), String::new()); }
    (hwnd_class(hwnd), hwnd_title(hwnd))
}

pub fn focus_hwnd(hwnd: HWND) -> Result<()> {
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        if !SetForegroundWindow(hwnd).as_bool() {
            return Err(anyhow!("SetForegroundWindow failed"));
        }
    }
    Ok(())
}

/// Focus + verify foreground + SendInput. Returns Err if foreground check fails.
pub fn send_keys_safe(hwnd: HWND, text: &str) -> Result<()> {
    focus_hwnd(hwnd)?;
    sleep(Duration::from_millis(30));
    let fg = unsafe { GetForegroundWindow() };
    if fg != hwnd {
        return Err(anyhow!("foreground verification failed"));
    }
    send_keys_raw(text);
    Ok(())
}

fn send_keys_raw(text: &str) {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(text.len() * 2);
    for ch in text.chars() {
        if ch == '\n' {
            inputs.push(kb_input(VK_RETURN, 0, false));
            inputs.push(kb_input(VK_RETURN, 0, true));
        } else {
            let code = ch as u16;
            inputs.push(kb_input(VIRTUAL_KEY(0), code, false));
            inputs.push(kb_input(VIRTUAL_KEY(0), code, true));
        }
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn kb_input(vk: VIRTUAL_KEY, scan: u16, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if scan != 0 { flags |= KEYEVENTF_UNICODE; }
    if up { flags |= KEYEVENTF_KEYUP; }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0,
            },
        },
    }
}
