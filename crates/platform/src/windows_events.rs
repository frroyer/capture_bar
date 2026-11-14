//! The event log on Windows (US-10.4): low-level mouse and keyboard hooks, and a 30 Hz
//! timer that samples the cursor and the active window, all on one thread with its own
//! message loop. Each input is placed on the session clock as it comes in and handed to
//! a writer thread, so the hooks return at once whatever the disk does. A key that types
//! leaves the hook as [`Key::Text`] and nothing else (DR-6).

use std::cell::RefCell;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use cb_core::events::{Button, EventLog, Input, Key};
use cb_core::MediaClock;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyNameTextW, MapVirtualKeyW, MAPVK_VK_TO_CHAR, VIRTUAL_KEY, VK_BACK,
    VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME,
    VK_INSERT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMLOCK,
    VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    VK_SNAPSHOT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowTextW,
    GetWindowThreadProcessId, KillTimer, PeekMessageW, PostThreadMessageW, SetTimer,
    SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
    MSLLHOOKSTRUCT, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_QUIT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WM_USER, WM_XBUTTONDOWN,
    WM_XBUTTONUP,
};

/// How often the cursor and the active window are sampled: about 30 Hz (DR-5). Timers
/// tick every 15.6 ms unless an app asks for finer: 33 ms would wait for a third tick
/// (21 Hz), 31 ms takes two (32 Hz).
const SAMPLE_MS: u32 = 31;

/// Stops the hooks when dropped; the writer then logs what it holds and closes the file.
pub struct Recorder {
    thread: u32,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // SAFETY: posting to a thread id; a gone thread only makes it fail.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

pub fn start(path: PathBuf, clock: MediaClock) -> std::io::Result<Recorder> {
    let (inputs, received) = mpsc::channel();
    std::thread::Builder::new()
        .name("event-log-writer".into())
        .spawn(move || write(path, received))?;
    let (id, thread) = mpsc::channel();
    std::thread::Builder::new()
        .name("event-log".into())
        .spawn(move || hook(clock, inputs, id))?;
    let thread = thread
        .recv()
        .map_err(|_| std::io::Error::other("the event thread ended"))?;
    Ok(Recorder { thread })
}

fn write(path: PathBuf, inputs: mpsc::Receiver<(u64, Input)>) {
    let file = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| File::create(&path));
    let mut log = match file {
        Ok(file) => EventLog::new(BufWriter::new(file)),
        Err(error) => return tracing::warn!(%error, path = %path.display(), "event log"),
    };
    for (t_ms, input) in inputs {
        if let Err(error) = log.push(t_ms, input) {
            return tracing::warn!(%error, "event log");
        }
    }
    if let Err(error) = log.finish() {
        tracing::warn!(%error, "event log");
    }
}

/// The hook thread's state, for the hook procedures, which get no context.
struct Hooked {
    clock: MediaClock,
    inputs: mpsc::Sender<(u64, Input)>,
    /// Keys held down, to tell a key's auto-repeat from a new press.
    down: [bool; 256],
    /// The active window last logged, and the process name of the last one seen.
    window: Option<(String, String)>,
    app: (u32, String),
}

thread_local! {
    static HOOKED: RefCell<Option<Hooked>> = const { RefCell::new(None) };
}

impl Hooked {
    /// Log `input` seen at `at`, if that is on the recording: not before its first frame
    /// or during a pause.
    fn send(&self, at: Instant, input: Input) -> bool {
        match (self.clock)(at) {
            Some(t) => self.inputs.send((t.as_millis() as u64, input)).is_ok(),
            None => false,
        }
    }
}

fn hook(clock: MediaClock, inputs: mpsc::Sender<(u64, Input)>, id: mpsc::Sender<u32>) {
    let mut msg = MSG::default();
    // SAFETY: plain Win32 calls on this thread; the hooks and the timer are removed
    // before it ends, and the state they read lives in its thread-local.
    unsafe {
        // Makes the thread's message queue, so the quit message can be posted to it.
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        HOOKED.set(Some(Hooked {
            clock,
            inputs,
            down: [false; 256],
            window: None,
            app: (0, String::new()),
        }));
        let _ = id.send(GetCurrentThreadId());
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), None, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), None, 0);
        if let Err(error) = keyboard.as_ref().and(mouse.as_ref()) {
            tracing::warn!(%error, "event hooks");
        }
        let timer = SetTimer(None, 0, SAMPLE_MS, None);
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER {
                sample();
            }
        }
        let _ = KillTimer(None, timer);
        for hook in [keyboard, mouse].into_iter().flatten() {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
    HOOKED.set(None);
}

/// The cursor and the active window, every [`SAMPLE_MS`].
fn sample() {
    let now = Instant::now();
    let mut point = POINT::default();
    // SAFETY: an out pointer to a local.
    let cursor = unsafe { GetCursorPos(&mut point) }.is_ok();
    HOOKED.with_borrow_mut(|hooked| {
        let Some(hooked) = hooked else { return };
        if cursor {
            hooked.send(
                now,
                Input::Cursor {
                    x: point.x,
                    y: point.y,
                },
            );
        }
        let Some(window) = foreground(&mut hooked.app) else {
            return;
        };
        if hooked.window.as_ref() != Some(&window) {
            let (title, app) = window.clone();
            if hooked.send(now, Input::Window { title, app }) {
                hooked.window = Some(window);
            }
        }
    });
}

/// The active window's title and its program's file name, kept in `app` by process.
fn foreground(app: &mut (u32, String)) -> Option<(String, String)> {
    // SAFETY: plain queries with out pointers to locals; the process handle is closed.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() {
            return None;
        }
        let mut title = [0u16; 512];
        let len = GetWindowTextW(window, &mut title).max(0) as usize;
        let title = String::from_utf16_lossy(&title[..len]);
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        if app.0 != pid || app.1.is_empty() {
            let mut name = String::new();
            if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut path = [0u16; 1024];
                let mut len = path.len() as u32;
                if QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &mut len,
                )
                .is_ok()
                {
                    let path = String::from_utf16_lossy(&path[..len as usize]);
                    name = path.rsplit('\\').next().unwrap_or_default().to_owned();
                }
                let _ = CloseHandle(process);
            }
            *app = (pid, name);
        }
        Some((title, app.1.clone()))
    }
}

unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let now = Instant::now();
        // SAFETY: for HC_ACTION, `lparam` points at the event's MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if let Some(input) = mouse_input(wparam.0 as u32, info) {
            HOOKED.with_borrow(|hooked| {
                if let Some(hooked) = hooked {
                    hooked.send(now, input);
                }
            });
        }
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn mouse_input(message: u32, info: &MSLLHOOKSTRUCT) -> Option<Input> {
    let (x, y) = (info.pt.x, info.pt.y);
    let high = (info.mouseData >> 16) as u16;
    let notches = f32::from(high as i16) / 120.0;
    let x_button = if high == 1 { Button::X1 } else { Button::X2 };
    let (button, down) = match message {
        WM_LBUTTONDOWN => (Button::Left, true),
        WM_LBUTTONUP => (Button::Left, false),
        WM_RBUTTONDOWN => (Button::Right, true),
        WM_RBUTTONUP => (Button::Right, false),
        WM_MBUTTONDOWN => (Button::Middle, true),
        WM_MBUTTONUP => (Button::Middle, false),
        WM_XBUTTONDOWN => (x_button, true),
        WM_XBUTTONUP => (x_button, false),
        WM_MOUSEWHEEL => {
            return Some(Input::Scroll {
                dx: 0.0,
                dy: notches,
                x,
                y,
            })
        }
        WM_MOUSEHWHEEL => {
            return Some(Input::Scroll {
                dx: notches,
                dy: 0.0,
                x,
                y,
            })
        }
        _ => return None,
    };
    Some(Input::Button { button, down, x, y })
}

unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let now = Instant::now();
        // SAFETY: for HC_ACTION, `lparam` points at the event's KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let pressed = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let released = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        let vk = (info.vkCode & 0xFF) as usize;
        HOOKED.with_borrow_mut(|hooked| {
            let Some(hooked) = hooked else { return };
            if released {
                hooked.down[vk] = false;
            }
            if !pressed {
                return;
            }
            let repeat = std::mem::replace(&mut hooked.down[vk], true);
            if let Some(key) = key(info) {
                // A held key types again and again; a held shortcut is one press.
                if !repeat || key == Key::Text || key == Key::Erase {
                    hooked.send(now, Input::Key(key));
                }
            }
        });
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The modifiers held with a key.
#[derive(Debug, Clone, Copy, Default)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    /// Right Alt: AltGr on the layouts that have it, which types.
    alt_gr: bool,
}

/// `info`'s key, classed; `None` for a modifier alone.
fn key(info: &KBDLLHOOKSTRUCT) -> Option<Key> {
    let vk = VIRTUAL_KEY(info.vkCode as u16);
    let modifiers = [
        VK_SHIFT,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_CONTROL,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_MENU,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
        VK_CAPITAL,
        VK_NUMLOCK,
    ];
    if modifiers.contains(&vk) {
        return None;
    }
    // SAFETY: plain state queries.
    let held = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(i32::from(k.0)) } < 0;
    let mods = Mods {
        ctrl: held(VK_CONTROL),
        alt: held(VK_MENU),
        shift: held(VK_SHIFT),
        win: held(VK_LWIN) || held(VK_RWIN),
        alt_gr: held(VK_RMENU),
    };
    // The character the key types on the keyboard layout, if any; dead keys too.
    // SAFETY: a plain query.
    let char = unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_CHAR) } & 0xFFFF;
    let numpad = (0x60..=0x6F).contains(&vk.0);
    Some(classify(vk, mods, char >= 0x20 || numpad, || {
        key_name(vk, info)
    }))
}

/// DR-6: whatever types is [`Key::Text`]. That is a key with a character and no Ctrl,
/// Alt or Win, or with AltGr or Ctrl+Alt, which type on many layouts (`@`, `€`). Only
/// the other keys are named.
fn classify(vk: VIRTUAL_KEY, m: Mods, types: bool, name: impl FnOnce() -> String) -> Key {
    let command = m.ctrl || m.alt || m.win;
    if types && (!command || m.alt_gr || (m.ctrl && m.alt && !m.win)) {
        return Key::Text;
    }
    if (vk == VK_BACK || vk == VK_DELETE) && !command {
        return Key::Erase;
    }
    let mut keys = String::new();
    for (on, label) in [
        (m.ctrl, "Ctrl+"),
        (m.alt, "Alt+"),
        (m.shift, "Shift+"),
        (m.win, "Win+"),
    ] {
        if on {
            keys.push_str(label);
        }
    }
    keys.push_str(&name());
    Key::Named(keys)
}

fn key_name(vk: VIRTUAL_KEY, info: &KBDLLHOOKSTRUCT) -> String {
    let named = match vk {
        VK_RETURN => "Enter",
        VK_ESCAPE => "Esc",
        VK_TAB => "Tab",
        VK_SPACE => "Space",
        VK_BACK => "Backspace",
        VK_DELETE => "Delete",
        VK_INSERT => "Insert",
        VK_HOME => "Home",
        VK_END => "End",
        VK_PRIOR => "PageUp",
        VK_NEXT => "PageDown",
        VK_LEFT => "Left",
        VK_RIGHT => "Right",
        VK_UP => "Up",
        VK_DOWN => "Down",
        VK_SNAPSHOT => "PrintScreen",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_owned();
    }
    match vk.0 {
        0x30..=0x39 | 0x41..=0x5A => return char::from(vk.0 as u8).to_string(),
        f if (VK_F1.0..=VK_F24.0).contains(&f) => return format!("F{}", f - VK_F1.0 + 1),
        _ => {}
    }
    let extended = u32::from(info.flags.0 & LLKHF_EXTENDED.0 != 0);
    let lparam = (info.scanCode << 16 | extended << 24) as i32;
    let mut name = [0u16; 64];
    // SAFETY: a plain query into a local buffer.
    let len = unsafe { GetKeyNameTextW(lparam, &mut name) }.max(0) as usize;
    match String::from_utf16_lossy(&name[..len]) {
        name if name.is_empty() => format!("VK{:02X}", vk.0),
        name => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(vk: VIRTUAL_KEY, m: Mods, types: bool) -> Key {
        classify(vk, m, types, || "N".into())
    }

    #[test]
    fn keys_that_type_are_never_named() {
        let a = VIRTUAL_KEY(0x41);
        let none = Mods::default();
        let shift = Mods {
            shift: true,
            ..none
        };
        let ctrl = Mods { ctrl: true, ..none };
        let alt_gr = Mods {
            ctrl: true,
            alt: true,
            alt_gr: true,
            ..none
        };
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..none
        };
        assert_eq!(class(a, none, true), Key::Text);
        assert_eq!(class(a, shift, true), Key::Text);
        assert_eq!(class(a, alt_gr, true), Key::Text);
        assert_eq!(class(a, ctrl_alt, true), Key::Text);
        assert_eq!(class(VK_SPACE, none, true), Key::Text);
        assert_eq!(class(a, ctrl, true), Key::Named("Ctrl+N".into()));
        let ctrl_shift = Mods {
            shift: true,
            ..ctrl
        };
        assert_eq!(
            class(a, ctrl_shift, true),
            Key::Named("Ctrl+Shift+N".into())
        );
        assert_eq!(class(VK_BACK, shift, false), Key::Erase);
        assert_eq!(class(VK_BACK, ctrl, false), Key::Named("Ctrl+N".into()));
        assert_eq!(class(VK_RETURN, none, false), Key::Named("N".into()));
    }

    /// Needs a desktop: logs the cursor and the active window for real.
    #[test]
    #[ignore]
    fn logs_the_cursor_and_the_window() {
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        let path = std::env::temp_dir().join("takebox-events-test.jsonl");
        let origin = Instant::now();
        let clock: MediaClock = std::sync::Arc::new(move |at| at.checked_duration_since(origin));
        let recorder = start(path.clone(), clock).unwrap();
        for i in 0..5 {
            // SAFETY: moves the cursor.
            let _ = unsafe { SetCursorPos(200 + i * 40, 200) };
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        drop(recorder);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let log = std::fs::read_to_string(&path).unwrap();
        println!("{log}");
        assert!(
            log.lines().filter(|l| l.contains("\"cursor\"")).count() >= 4,
            "{log}"
        );
        assert!(log.contains("\"window\""), "{log}");
    }
}
