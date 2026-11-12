//! FR-6, US-2.1: an excluded window is absent from a real screen grab.
//!
//! Needs an interactive desktop, so it does not run by default:
//!   cargo test -p cb-platform --test exclude_from_capture -- --ignored
//!
//! The grab is only used to count pixels. Nothing is saved.
#![cfg(windows)]

use cb_core::CaptureTarget;
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use std::num::NonZeroIsize;
use std::time::Duration;
use std::time::Instant;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, HBRUSH};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetWindowRect,
    PeekMessageW, RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};

const SIDE: i32 = 300;

struct Window(HWND);

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: the window outlives the borrow; it is destroyed only at the end of the test.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)))
        })
    }
}

fn magenta_pixels(source: &dyn cb_core::ScreenSource, target: &CaptureTarget) -> usize {
    let frame = source.grab(target).expect("grab");
    frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 245 && p[1] < 10 && p[2] > 245)
        .count()
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: forwards the arguments the system passed in.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Lets the window paint its background.
fn pump_messages(duration: Duration) {
    let start = Instant::now();
    while start.elapsed() < duration {
        let mut msg = MSG::default();
        // SAFETY: `msg` is a valid out-parameter; messages belong to this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "needs an interactive desktop"]
fn excluded_window_is_absent_from_grab() {
    let source = cb_platform::screen_source();
    let display = source.displays().expect("displays").remove(0);
    let target = CaptureTarget::Display(display.id);

    // SAFETY: plain Win32 calls; the class name and brush live for the whole process.
    let hwnd = unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: w!("TakeboxExcludeTest"),
            hbrBackground: HBRUSH(CreateSolidBrush(COLORREF(0x00FF_00FF)).0),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&class), 0, "register class");
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("TakeboxExcludeTest"),
            w!("exclude_from_capture test"),
            WS_POPUP | WS_VISIBLE,
            display.bounds.x + 100,
            display.bounds.y + 100,
            SIDE,
            SIDE,
            None,
            None,
            None,
            None,
        )
        .expect("create window")
    };
    let window = Window(hwnd);
    pump_messages(Duration::from_millis(300));

    // Proves the check can fail: the window shows up before exclusion.
    let before = magenta_pixels(source.as_ref(), &target);
    cb_platform::exclude_from_capture(&window).expect("exclude");
    pump_messages(Duration::from_millis(300));
    let after = magenta_pixels(source.as_ref(), &target);

    // SAFETY: created above, on this thread.
    unsafe { DestroyWindow(hwnd) }.expect("destroy window");

    assert!(before > 10_000, "window not visible first: {before} pixels");
    assert!(
        after < before / 100,
        "window still captured: {before} -> {after} pixels"
    );
}

/// The strip winit leaves at the top must go: it shows behind the bar's rounded corners.
#[test]
#[ignore = "needs an interactive desktop"]
fn removing_the_frame_makes_client_area_fill_the_window() {
    // SAFETY: plain Win32 calls on a window created and destroyed here, on this thread.
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            lpszClassName: w!("TakeboxFrameTest"),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&class), 0, "register class");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("TakeboxFrameTest"),
            w!("remove_window_frame test"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            SIDE,
            SIDE,
            None,
            None,
            None,
            None,
        )
        .expect("create window");
        let sizes = || {
            let (mut outer, mut client) = (RECT::default(), RECT::default());
            GetWindowRect(hwnd, &mut outer).expect("window rect");
            GetClientRect(hwnd, &mut client).expect("client rect");
            (outer.bottom - outer.top, client.bottom)
        };

        let (outer, client) = sizes();
        assert!(
            client < outer,
            "a framed window starts with a smaller client area"
        );
        cb_platform::remove_window_frame(&Window(hwnd)).expect("remove frame");
        let (outer, client) = sizes();
        DestroyWindow(hwnd).expect("destroy window");
        assert_eq!(client, outer);
    }
}
