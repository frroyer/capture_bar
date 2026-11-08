//! Launch at login (US-8.3, FR-29). The OS entry itself is the setting: nothing is kept in
//! the settings file, so it never disagrees with what the OS will do.
//!
//! - Windows: a value in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//! - macOS: `~/Library/LaunchAgents/<LABEL>.plist` with `RunAtLoad`.
//! - Linux: `$XDG_CONFIG_HOME/autostart/capture-bar.desktop` (XDG autostart).

use std::io;
use std::path::PathBuf;

/// What the OS should start: this binary, or on Linux the AppImage that holds it, since the
/// binary's own path only exists while the AppImage runs.
fn program() -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Ok(appimage.into());
    }
    std::env::current_exe()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Capture Bar";

    fn check(e: WIN32_ERROR) -> io::Result<()> {
        if e.is_ok() {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(e.0 as i32))
        }
    }

    pub fn is_set(key: &str, value: &str) -> bool {
        // SAFETY: a plain value lookup; no output buffer is passed.
        let e = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                &HSTRING::from(value),
                RRF_RT_REG_SZ,
                None,
                None,
                None,
            )
        };
        e.is_ok()
    }

    /// Write `command` as `value` under `key`, or remove the value when `None`.
    pub fn set(key: &str, value: &str, command: Option<&str>) -> io::Result<()> {
        let (key, value) = (HSTRING::from(key), HSTRING::from(value));
        match command {
            Some(command) => {
                let data: Vec<u16> = command.encode_utf16().chain([0]).collect();
                // SAFETY: `data` is a NUL-terminated UTF-16 string that outlives the call.
                check(unsafe {
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        &key,
                        &value,
                        REG_SZ.0,
                        Some(data.as_ptr().cast()),
                        (data.len() * 2) as u32,
                    )
                })
            }
            None => {
                // SAFETY: a plain value removal.
                let e = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &key, &value) };
                if e == ERROR_FILE_NOT_FOUND {
                    return Ok(());
                }
                check(e)
            }
        }
    }

    pub fn enabled() -> bool {
        is_set(RUN_KEY, VALUE)
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let command = on
            .then(|| program().map(|p| format!("\"{}\"", p.display())))
            .transpose()?;
        set(RUN_KEY, VALUE, command.as_deref())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::path::Path;

    #[cfg(target_os = "macos")]
    const LABEL: &str = "app.capturebar.CaptureBar";

    fn home() -> io::Result<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME"))
    }

    #[cfg(target_os = "macos")]
    fn entry() -> io::Result<PathBuf> {
        Ok(home()?
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    #[cfg(not(target_os = "macos"))]
    fn entry() -> io::Result<PathBuf> {
        let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => home()?.join(".config"),
        };
        Ok(config.join("autostart").join("capture-bar.desktop"))
    }

    #[cfg(target_os = "macos")]
    pub fn contents(program: &Path) -> String {
        let escaped = program
            .display()
            .to_string()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{escaped}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
        )
    }

    /// A desktop entry; its Exec path is in double quotes, with the characters the
    /// Desktop Entry spec reserves there escaped.
    #[cfg(not(target_os = "macos"))]
    pub fn contents(program: &Path) -> String {
        let mut quoted = String::new();
        for c in program.display().to_string().chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        format!(
            "[Desktop Entry]\nType=Application\nName=Capture Bar\nExec=\"{quoted}\"\nX-GNOME-Autostart-enabled=true\n"
        )
    }

    pub fn enabled() -> bool {
        entry().is_ok_and(|e| e.is_file())
    }

    pub fn set_enabled(on: bool) -> io::Result<()> {
        let entry = entry()?;
        if on {
            std::fs::create_dir_all(entry.parent().expect("entry has a folder"))?;
            std::fs::write(&entry, contents(&program()?))
        } else {
            match std::fs::remove_file(&entry) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

/// Whether the bar starts at login.
pub fn enabled() -> bool {
    imp::enabled()
}

/// Register or unregister the bar to start at login.
pub fn set_enabled(on: bool) -> io::Result<()> {
    imp::set_enabled(on)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn a_registry_value_is_set_and_removed() {
        use super::imp::{is_set, set};
        let key = r"Software\Capture Bar tests";
        let value = format!("autostart {}", std::process::id());
        set(key, &value, Some(r#""C:\a b\capture-bar.exe""#)).unwrap();
        assert!(is_set(key, &value));
        set(key, &value, None).unwrap();
        assert!(!is_set(key, &value));
        set(key, &value, None).expect("removing what is not there is fine");
        use windows::Win32::System::Registry::{RegDeleteKeyW, HKEY_CURRENT_USER};
        // SAFETY: removes the empty test key made above.
        let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, &windows::core::HSTRING::from(key)) };
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn the_desktop_entry_quotes_the_path() {
        let entry = super::imp::contents(std::path::Path::new("/home/a b/$x/capture-bar"));
        assert!(entry.contains("Exec=\"/home/a b/\\$x/capture-bar\"\n"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_launch_agent_escapes_the_path() {
        let plist = super::imp::contents(std::path::Path::new("/Apps/A&B.app/x"));
        assert!(plist.contains("<string>/Apps/A&amp;B.app/x</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
    }
}
