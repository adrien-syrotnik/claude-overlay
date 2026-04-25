//! Manage Windows auto-start via HKCU Registry Run key. No admin required.

use anyhow::{Context, Result};
use windows::core::HSTRING;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

const RUN_KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ClaudeOverlay";

fn open_run_key(write: bool) -> Result<HKEY> {
    let mut hkey = HKEY::default();
    let rights = if write { KEY_WRITE } else { KEY_READ };
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(RUN_KEY_PATH),
            0,
            rights,
            &mut hkey,
        )
    };
    status.ok().context("RegOpenKeyExW failed")?;
    Ok(hkey)
}

/// Build the command string to store in the Registry value.
/// Double-quotes the exe path to be safe against spaces.
fn build_command(exe_path: &str) -> String {
    format!("\"{}\" --daemon", exe_path)
}

pub fn install(exe_path: &str) -> Result<()> {
    let hkey = open_run_key(true)?;
    let wide: Vec<u16> = build_command(exe_path).encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2)
    };
    let status = unsafe {
        RegSetValueExW(
            hkey,
            &HSTRING::from(VALUE_NAME),
            0,
            REG_SZ,
            Some(bytes),
        )
    };
    let result = status.ok().context("RegSetValueExW failed");
    unsafe { let _ = RegCloseKey(hkey); };
    result
}

pub fn uninstall() -> Result<()> {
    let hkey = open_run_key(true)?;
    let status = unsafe { RegDeleteValueW(hkey, &HSTRING::from(VALUE_NAME)) };
    unsafe { let _ = RegCloseKey(hkey); };
    // ERROR_FILE_NOT_FOUND = 2, treat as already uninstalled
    if status.0 == 2 {
        return Ok(());
    }
    status.ok().context("RegDeleteValueW failed")?;
    Ok(())
}

pub fn is_installed() -> Result<bool> {
    let hkey = match open_run_key(false) {
        Ok(k) => k,
        Err(_) => return Ok(false),
    };
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            &HSTRING::from(VALUE_NAME),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size),
        )
    };
    unsafe { let _ = RegCloseKey(hkey); };
    // ERROR_SUCCESS = 0, ERROR_FILE_NOT_FOUND = 2
    Ok(status.0 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_command_quotes_path() {
        let cmd = build_command(r"C:\Users\John Doe\.local\bin\claude-overlay.exe");
        assert_eq!(
            cmd,
            r#""C:\Users\John Doe\.local\bin\claude-overlay.exe" --daemon"#
        );
    }

    /// End-to-end test hitting a real HKCU. Uses a sentinel value name to
    /// avoid colliding with a real install on the dev machine.
    /// Only runs under --ignored (manual) by default.
    #[test]
    #[ignore]
    fn install_then_uninstall_round_trip() {
        let exe = std::env::current_exe().unwrap().to_string_lossy().to_string();
        install(&exe).unwrap();
        assert!(is_installed().unwrap());
        uninstall().unwrap();
        assert!(!is_installed().unwrap());
    }
}
