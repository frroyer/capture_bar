//! Windows backend.
//! Capture: Windows.Graphics.Capture (one session per grab, polled free-threaded).
//! Encode:  Media Foundation H.264 from GPU textures, see `windows_record` (US-5.1).
//! Exclude the bar from captures: SetWindowDisplayAffinity (US-2.1).

use cb_core::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use windows::core::{factory, Interface, Result as WinResult};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetShellWindow, GetSystemMetrics, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SetWindowDisplayAffinity,
    SetWindowPos, SM_SWAPBUTTON, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, WDA_EXCLUDEFROMCAPTURE, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCPAINT,
};

/// How long to wait for the first frame of a capture session.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) fn platform_err(e: windows::core::Error) -> CaptureError {
    CaptureError::Platform(e.to_string())
}

fn hwnd_of(window: &impl raw_window_handle::HasWindowHandle) -> CaptureResult<HWND> {
    let handle = window
        .window_handle()
        .map_err(|e| CaptureError::Platform(e.to_string()))?;
    let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(CaptureError::Platform("not a Win32 window".into()));
    };
    Ok(HWND(handle.hwnd.get() as *mut core::ffi::c_void))
}

/// Call from the thread that owns the window.
pub(crate) fn exclude_from_capture(
    window: &impl raw_window_handle::HasWindowHandle,
) -> CaptureResult<()> {
    let hwnd = hwnd_of(window)?;
    // SAFETY: `hwnd` comes from a live window borrowed for this call.
    unsafe { SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) }.map_err(platform_err)
}

/// This process's top-level window titled `title`, as in [`window_id`].
pub(crate) fn own_window(title: &str) -> Option<isize> {
    use windows::core::HSTRING;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    // SAFETY: the title outlives the call; a stale handle only makes the second fail.
    unsafe {
        let hwnd = FindWindowW(None, &HSTRING::from(title)).ok()?;
        let mut process = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process));
        (process == GetCurrentProcessId()).then_some(hwnd.0 as isize)
    }
}

/// The window's handle as a number, which can cross threads: see [`crate::Restorer`].
pub(crate) fn window_id(window: &impl raw_window_handle::HasWindowHandle) -> Option<isize> {
    hwnd_of(window).ok().map(|hwnd| hwnd.0 as isize)
}

/// See [`crate::Restorer`].
pub(crate) fn restore_if_minimized(id: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, ShowWindowAsync, SW_RESTORE};
    let hwnd = HWND(id as *mut _);
    // SAFETY: both calls accept any value; a stale handle only makes them fail.
    // Async: the window's thread does the work, so this never blocks the caller.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
    }
}

/// See [`crate::show_in_taskbar`].
pub(crate) fn show_in_taskbar(
    window: &impl raw_window_handle::HasWindowHandle,
    show: bool,
) -> CaptureResult<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
    let hwnd = hwnd_of(window)?;
    // SAFETY: plain COM calls on the window's thread, where winit initialised COM;
    // `hwnd` comes from a live window borrowed for this call.
    unsafe {
        let list: ITaskbarList =
            CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).map_err(platform_err)?;
        list.HrInit().map_err(platform_err)?;
        if show {
            list.AddTab(hwnd)
        } else {
            list.DeleteTab(hwnd)
        }
        .map_err(platform_err)
    }
}

/// winit keeps a 1 px strip above the client area of borderless windows, which
/// shows wherever the bar's rounded corners leave the window transparent. Answering
/// the message here makes the client area cover the whole window.
///
/// With DWM's frame turned off, Windows paints the classic frame itself on
/// WM_NCPAINT and WM_NCACTIVATE, right over the content: a white title bar flashed
/// on the bar each time activation moved between it and the camera bubble. There is
/// no frame to paint: WM_NCPAINT is answered here, and WM_NCACTIVATE goes on with
/// lParam -1, which keeps Windows' own activation bookkeeping (window moves need
/// it) but skips the repaint.
unsafe extern "system" fn fill_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 => return LRESULT(0),
        WM_NCPAINT => return LRESULT(0),
        WM_NCACTIVATE => {
            // SAFETY: forwards the system's message, without the frame repaint.
            return unsafe { DefSubclassProc(hwnd, msg, wparam, LPARAM(-1)) };
        }
        _ => {}
    }
    // SAFETY: forwards the arguments the system passed in.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Turns off the drop shadow egui-winit requests for borderless windows, the
/// strip winit leaves at the top, and Windows 11's rounded clipping, which would
/// cut the bar's own corners. Call from the thread that owns the window.
pub(crate) fn remove_window_frame(
    window: &impl raw_window_handle::HasWindowHandle,
) -> CaptureResult<()> {
    let hwnd = hwnd_of(window)?;
    // SAFETY: `hwnd` comes from a live window; each pointer is valid for its stated size.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            std::ptr::from_ref(&DWMNCRP_DISABLED).cast(),
            size_of_val(&DWMNCRP_DISABLED) as u32,
        )
        .map_err(platform_err)?;
        // Windows 10 has no such attribute and no rounding to turn off.
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&DWMWCP_DONOTROUND).cast(),
            size_of_val(&DWMWCP_DONOTROUND) as u32,
        );
        if !SetWindowSubclass(hwnd, Some(fill_window_proc), 1, 0).as_bool() {
            return Err(CaptureError::Platform("SetWindowSubclass failed".into()));
        }
        // Makes Windows ask for the new client area straight away.
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(platform_err)?;
    }
    Ok(())
}

pub(crate) fn primary_button_down() -> bool {
    // SAFETY: neither call has preconditions.
    unsafe {
        let key = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        GetAsyncKeyState(i32::from(key.0)) < 0
    }
}

pub(crate) fn system_ui_fonts() -> Option<(Vec<u8>, Vec<u8>)> {
    let fonts = std::path::PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts");
    Some((
        std::fs::read(fonts.join("segoeui.ttf")).ok()?,
        std::fs::read(fonts.join("seguisb.ttf")).ok()?,
    ))
}

/// WinRT and Media Foundation need a multithreaded apartment; keep one alive
/// for the whole process so worker threads need no per-thread setup.
pub(crate) fn ensure_mta() {
    static MTA: OnceLock<()> = OnceLock::new();
    MTA.get_or_init(|| {
        // SAFETY: no preconditions. The cookie is dropped on purpose: never revoked.
        if let Err(e) = unsafe { CoIncrementMTAUsage() } {
            tracing::warn!("CoIncrementMTAUsage failed: {e}");
        }
    });
}

/// One D3D11 device for the process: creating it costs ~75 ms per grab (NFR-4).
pub(crate) struct SharedGpu {
    pub device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

// SAFETY: the device is multithread-protected below, which also serialises the
// immediate context; the WinRT wrapper is free-threaded.
unsafe impl Send for SharedGpu {}
unsafe impl Sync for SharedGpu {}

// chisle: no device-lost recovery (driver reset breaks grabs until relaunch); add in US-4.4.
pub(crate) fn shared_gpu() -> WinResult<&'static SharedGpu> {
    static GPU: OnceLock<WinResult<SharedGpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        // SAFETY: out-pointers are valid for the duration of the call.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.expect("D3D11CreateDevice succeeded without a device");
        let context = context.expect("D3D11CreateDevice succeeded without a context");
        // Capture workers and the encoder use this device from their own threads.
        // SAFETY: plain flag setter on a live device.
        let _ = unsafe {
            device
                .cast::<ID3D11Multithread>()?
                .SetMultithreadProtected(true)
        };
        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: `dxgi` is a live DXGI device.
        let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        Ok(SharedGpu {
            device,
            context,
            winrt_device,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Capture items per monitor: creating one costs ~77 ms per grab (NFR-4).
fn capture_items() -> &'static Mutex<HashMap<u64, GraphicsCaptureItem>> {
    static ITEMS: OnceLock<Mutex<HashMap<u64, GraphicsCaptureItem>>> = OnceLock::new();
    ITEMS.get_or_init(Default::default)
}

pub(crate) fn capture_item(monitor: HMONITOR) -> WinResult<GraphicsCaptureItem> {
    ensure_mta();
    let mut items = capture_items().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(item) = items.get(&(monitor.0 as u64)) {
        return Ok(item.clone());
    }
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    // SAFETY: `monitor` comes from the Win32 monitor APIs.
    let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(monitor)? };
    items.insert(monitor.0 as u64, item.clone());
    Ok(item)
}

fn forget_capture_item(monitor: HMONITOR) {
    let mut items = capture_items().lock().unwrap_or_else(|e| e.into_inner());
    items.remove(&(monitor.0 as u64));
}

/// A running capture session on one monitor or window, with a CPU-readable copy of the newest frame.
pub(crate) struct MonitorCapture {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    staging: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
}

impl MonitorCapture {
    /// With the cursor in the frames, as a recording wants.
    pub fn start(monitor: HMONITOR) -> WinResult<Self> {
        Self::on_monitor(monitor, true)
    }

    fn on_monitor(monitor: HMONITOR, cursor: bool) -> WinResult<Self> {
        let started = Self::start_with(&capture_item(monitor)?, cursor);
        if started.is_err() {
            // The cached item may be stale (monitor unplugged): rebuild it on the next grab.
            forget_capture_item(monitor);
        }
        started
    }

    pub fn start_with(item: &GraphicsCaptureItem, cursor: bool) -> WinResult<Self> {
        let gpu = shared_gpu()?;
        let (device, context) = (gpu.device.clone(), gpu.context.clone());
        let size = item.Size()?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &gpu.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(item)?;
        // Not available on every Windows build; the yellow border is only cosmetic here.
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(cursor);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.Width as u32,
            Height: size.Height as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        // SAFETY: `desc` and the out-pointer are valid for the duration of the call.
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging))? };
        let staging = staging.expect("CreateTexture2D succeeded without a texture");

        session.StartCapture()?;
        Ok(Self {
            device,
            context,
            pool,
            session,
            staging,
            width: size.Width as u32,
            height: size.Height as u32,
        })
    }

    /// Drain the pool and copy the newest frame to the staging texture.
    /// Returns false if no new frame arrived since the last call.
    pub fn copy_latest(&self) -> WinResult<bool> {
        let Some(texture) = self.latest_texture()? else {
            return Ok(false);
        };
        // SAFETY: the texture is on our device, same size and format as staging.
        unsafe { self.context.CopyResource(&self.staging, &texture) };
        Ok(true)
    }

    /// Drain the pool: the newest frame's texture, if one arrived since the last call.
    /// Copy from it before the next call, which hands its buffer back to the pool.
    pub fn latest_texture(&self) -> WinResult<Option<ID3D11Texture2D>> {
        let mut latest = None;
        // TryGetNextFrame fails or returns null once the pool is empty.
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            latest = Some(frame);
        }
        let Some(frame) = latest else {
            return Ok(None);
        };
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: the surface of a frame from our pool is a D3D11 texture.
        Ok(Some(unsafe { access.GetInterface()? }))
    }

    /// Poll until the first frame is copied, or time out.
    pub fn wait_first_frame(&self) -> CaptureResult<()> {
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        while !self.copy_latest().map_err(platform_err)? {
            if Instant::now() > deadline {
                return Err(CaptureError::Platform("no frame within 1 s".into()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// Run `f` on the staging texture's BGRA pixels with their row pitch in bytes.
    pub fn with_pixels<T>(&self, f: impl FnOnce(&[u8], usize) -> T) -> WinResult<T> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: staging is CPU-readable; the slice is only used before Unmap.
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let bytes =
                std::slice::from_raw_parts(mapped.pData as *const u8, pitch * self.height as usize);
            let out = f(bytes, pitch);
            self.context.Unmap(&self.staging, 0);
            Ok(out)
        }
    }
}

impl Drop for MonitorCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Copy a sub-rectangle of a BGRA image to tightly packed, opaque RGBA.
fn bgra_to_rgba(bgra: &[u8], pitch: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in bgra.chunks_exact(pitch).skip(y).take(h) {
        for [b, g, r, _] in row[x * 4..(x + w) * 4].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[*r, *g, *b, 255]);
        }
    }
    rgba
}

fn enum_monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(
        m: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows::core::BOOL {
        // SAFETY: `data` is the &mut Vec passed below, alive for the whole enumeration.
        unsafe { (*(data.0 as *mut Vec<HMONITOR>)).push(m) };
        TRUE
    }
    let mut monitors: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only runs during this call.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut monitors as *mut _ as isize),
        )
    };
    monitors
}

fn monitor_bounds(monitor: HMONITOR) -> Option<(RECT, RECT, String)> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: cbSize announces the extended struct, which `info` is.
    let ok = unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) };
    ok.as_bool().then(|| {
        let name = String::from_utf16_lossy(&info.szDevice);
        (
            info.monitorInfo.rcMonitor,
            info.monitorInfo.rcWork,
            name.trim_end_matches('\0').to_string(),
        )
    })
}

/// `DisplayId` is the HMONITOR value; id 0 means the primary display.
pub(crate) fn resolve_display(id: DisplayId) -> CaptureResult<HMONITOR> {
    if id.0 == 0 {
        // SAFETY: no preconditions.
        return Ok(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) });
    }
    enum_monitors()
        .into_iter()
        .find(|m| m.0 as u64 == id.0)
        .ok_or(CaptureError::TargetGone)
}

pub(crate) fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: the out-pointer is valid for the duration of the call.
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}

/// A window the user can pick: on screen, titled, and not this app's.
fn window_info(hwnd: HWND, own_process: u32, shell: HWND) -> Option<WindowInfo> {
    let mut process = 0;
    // SAFETY: `hwnd` comes from EnumWindows; a stale handle only makes these calls fail.
    // The out-pointers are valid for each call and their sizes match.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut process));
        if process == own_process
            || hwnd == shell
            || !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
        {
            return None;
        }
        // Windows on other virtual desktops and suspended store apps are cloaked.
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .ok()?;
        if cloaked != 0 {
            return None;
        }
        let mut title = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut title) as usize;
        if len == 0 {
            return None;
        }
        // Without the invisible resize border and the shadow, unlike GetWindowRect.
        let mut r = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
        (r.right > r.left && r.bottom > r.top).then(|| WindowInfo {
            id: WindowId(hwnd.0 as u64),
            title: String::from_utf16_lossy(&title[..len]),
            app_name: String::new(),
            bounds: Rect {
                x: r.left,
                y: r.top,
                width: (r.right - r.left) as u32,
                height: (r.bottom - r.top) as u32,
            },
        })
    }
}

fn window_item(id: WindowId) -> CaptureResult<GraphicsCaptureItem> {
    ensure_mta();
    let hwnd = HWND(id.0 as *mut _);
    // SAFETY: IsWindow accepts any value.
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return Err(CaptureError::TargetGone);
    }
    let interop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(platform_err)?;
    // SAFETY: `hwnd` was just checked to be a window.
    unsafe { interop.CreateForWindow(hwnd) }.map_err(platform_err)
}

/// The monitor that holds most of `rect`.
fn monitor_of(rect: &Rect) -> HMONITOR {
    let r = RECT {
        left: rect.x,
        top: rect.y,
        right: rect.x + rect.width as i32,
        bottom: rect.y + rect.height as i32,
    };
    // SAFETY: `r` is valid for the duration of the call.
    unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST) }
}

/// The part of `rect` on `monitor`, in the pixels of a `width` x `height` capture of it:
/// (x, y, width, height).
fn crop_on_monitor(
    monitor: HMONITOR,
    rect: Rect,
    width: u32,
    height: u32,
) -> CaptureResult<(u32, u32, u32, u32)> {
    let (origin, ..) = monitor_bounds(monitor).ok_or(CaptureError::TargetGone)?;
    let left = (rect.x - origin.left).max(0) as u32;
    let top = (rect.y - origin.top).max(0) as u32;
    let right = ((rect.x - origin.left + rect.width as i32).max(0) as u32).min(width);
    let bottom = ((rect.y - origin.top + rect.height as i32).max(0) as u32).min(height);
    if right <= left || bottom <= top {
        return Err(CaptureError::Platform("region is off screen".into()));
    }
    Ok((left, top, right - left, bottom - top))
}

/// The top-left corner of a monitor in global physical pixels.
fn monitor_origin(monitor: HMONITOR) -> (i32, i32) {
    monitor_bounds(monitor).map_or((0, 0), |(r, ..)| (r.left, r.top))
}

/// The top-left corner of a window as captured, without its invisible border.
fn window_origin(id: WindowId) -> (i32, i32) {
    let mut r = RECT::default();
    // SAFETY: a stale handle only makes the call fail; the out-pointer fits a RECT.
    let found = unsafe {
        DwmGetWindowAttribute(
            HWND(id.0 as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    found.map_or((0, 0), |()| (r.left, r.top))
}

/// What a recording reads: a session with the cursor, and where in its frames.
pub(crate) struct RecordingSource {
    pub capture: MonitorCapture,
    /// The part of the frames to keep: (x, y, width, height).
    pub area: (u32, u32, u32, u32),
    /// The frames' top-left corner on the desktop, in global physical pixels.
    pub origin: (i32, i32),
}

/// A region across monitors keeps the part on the monitor that holds most of it.
pub(crate) fn recording_source(target: &CaptureTarget) -> CaptureResult<RecordingSource> {
    let (capture, region, origin) = match target {
        CaptureTarget::Display(id) => {
            let monitor = resolve_display(*id)?;
            (
                MonitorCapture::start(monitor).map_err(platform_err)?,
                None,
                monitor_origin(monitor),
            )
        }
        CaptureTarget::Region(rect) => {
            let monitor = monitor_of(rect);
            (
                MonitorCapture::start(monitor).map_err(platform_err)?,
                Some((monitor, *rect)),
                monitor_origin(monitor),
            )
        }
        CaptureTarget::Window(id) => (
            MonitorCapture::start_with(&window_item(*id)?, true).map_err(platform_err)?,
            None,
            window_origin(*id),
        ),
        CaptureTarget::Desktop => {
            return Err(CaptureError::NotImplemented(
                "recording every display at once",
            ))
        }
    };
    let area = match region {
        None => (0, 0, capture.width, capture.height),
        Some((monitor, rect)) => crop_on_monitor(monitor, rect, capture.width, capture.height)?,
    };
    Ok(RecordingSource {
        capture,
        area,
        origin,
    })
}

fn grab_window(id: WindowId) -> CaptureResult<Frame> {
    let capture = MonitorCapture::start_with(&window_item(id)?, false).map_err(platform_err)?;
    capture.wait_first_frame()?;
    let (w, h) = (capture.width, capture.height);
    let rgba = capture
        .with_pixels(|bgra, pitch| bgra_to_rgba(bgra, pitch, 0, 0, w as usize, h as usize))
        .map_err(platform_err)?;
    Ok(Frame {
        width: w,
        height: h,
        rgba,
        captured_at: SystemTime::now(),
    })
}

pub struct WinScreenSource;

impl WinScreenSource {
    pub fn new() -> Self {
        Self
    }
}

impl ScreenSource for WinScreenSource {
    fn displays(&self) -> CaptureResult<Vec<DisplayInfo>> {
        Ok(enum_monitors()
            .into_iter()
            .filter_map(|m| {
                let (r, work, name) = monitor_bounds(m)?;
                let (mut dpi_x, mut dpi_y) = (96, 96);
                // SAFETY: out-pointers are valid; on failure the 96 DPI default stays.
                let _ = unsafe { GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
                Some(DisplayInfo {
                    id: DisplayId(m.0 as u64),
                    name,
                    bounds: Rect {
                        x: r.left,
                        y: r.top,
                        width: (r.right - r.left) as u32,
                        height: (r.bottom - r.top) as u32,
                    },
                    work_area: Rect {
                        x: work.left,
                        y: work.top,
                        width: (work.right - work.left) as u32,
                        height: (work.bottom - work.top) as u32,
                    },
                    scale_factor: dpi_x as f32 / 96.0,
                })
            })
            .collect())
    }

    fn windows(&self) -> CaptureResult<Vec<WindowInfo>> {
        unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
            // SAFETY: `data` is the &mut Vec passed below, alive for the whole enumeration.
            unsafe { (*(data.0 as *mut Vec<HWND>)).push(hwnd) };
            TRUE
        }
        let mut handles: Vec<HWND> = Vec::new();
        // SAFETY: the callback only runs during this call. The order is front to back.
        unsafe { EnumWindows(Some(collect), LPARAM(&mut handles as *mut _ as isize)) }
            .map_err(platform_err)?;
        // SAFETY: no preconditions.
        let (own_process, shell) = unsafe { (GetCurrentProcessId(), GetShellWindow()) };
        Ok(handles
            .into_iter()
            .filter_map(|hwnd| window_info(hwnd, own_process, shell))
            .collect())
    }

    fn grab(&self, target: &CaptureTarget) -> CaptureResult<Frame> {
        let (monitor, region) = match target {
            CaptureTarget::Display(id) => (resolve_display(*id)?, None),
            CaptureTarget::Region(rect) => {
                let displays = self.displays()?;
                if displays.iter().filter(|d| d.bounds.overlaps(rect)).count() > 1 {
                    let desktop = desktop_bounds(&displays)
                        .ok_or_else(|| CaptureError::Platform("no display".into()))?;
                    return crop(
                        &self.grab(&CaptureTarget::Desktop)?,
                        (desktop.x, desktop.y),
                        *rect,
                    )
                    .ok_or_else(|| CaptureError::Platform("region is off screen".into()));
                }
                (monitor_of(rect), Some(*rect))
            }
            CaptureTarget::Window(id) => return grab_window(*id),
            CaptureTarget::Desktop => {
                let displays = self.displays()?;
                let desktop = desktop_bounds(&displays)
                    .ok_or_else(|| CaptureError::Platform("no display".into()))?;
                let parts = displays
                    .iter()
                    .map(|d| Ok((d.bounds, self.grab(&CaptureTarget::Display(d.id))?)))
                    .collect::<CaptureResult<Vec<_>>>()?;
                return Ok(stitch(&parts, desktop));
            }
        };

        let capture = MonitorCapture::on_monitor(monitor, false).map_err(platform_err)?;
        capture.wait_first_frame()?;
        let captured_at = SystemTime::now();

        // Crop to the part of the region on this monitor, in monitor coordinates.
        let (x, y, w, h) = match region {
            None => (0, 0, capture.width, capture.height),
            Some(rect) => crop_on_monitor(monitor, rect, capture.width, capture.height)?,
        };

        let rgba = capture
            .with_pixels(|bgra, pitch| {
                bgra_to_rgba(bgra, pitch, x as usize, y as usize, w as usize, h as usize)
            })
            .map_err(platform_err)?;
        Ok(Frame {
            width: w,
            height: h,
            rgba,
            captured_at,
        })
    }

    fn start_recording(
        &self,
        target: &CaptureTarget,
        fps: u32,
        audio: AudioSources,
        path: &std::path::Path,
        done: Box<dyn FnOnce(CaptureResult<Recording>) + Send>,
    ) -> CaptureResult<Box<dyn StreamHandle>> {
        Ok(crate::windows_record::start(
            target.clone(),
            fps,
            audio,
            path.to_path_buf(),
            done,
        ))
    }
}

/// See [`crate::drag_file_out`]. The shell builds the data object, the same one Explorer
/// would offer for the file (CF_HDROP and shell IDs), and a default drop source.
pub(crate) fn drag_file_out(path: &std::path::Path) -> CaptureResult<()> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{IDropSource, DROPEFFECT_COPY};
    use windows::Win32::UI::Shell::{
        BHID_DataObject, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop,
    };
    // SAFETY: plain COM calls on the UI thread, where winit initialised OLE for drag and drop.
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)
            .map_err(platform_err)?;
        let data: IDataObject = item
            .BindToHandler(None, &BHID_DataObject)
            .map_err(platform_err)?;
        SHDoDragDrop(None, &data, None::<&IDropSource>, DROPEFFECT_COPY).map_err(platform_err)?;
    }
    Ok(())
}

/// See [`crate::open_file`].
pub(crate) fn open_file(path: &std::path::Path) -> CaptureResult<()> {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // SAFETY: both strings outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success; the rest are error codes.
    match result.0 as isize {
        code if code > 32 => Ok(()),
        code => Err(CaptureError::Platform(format!(
            "could not open the file (error {code})"
        ))),
    }
}

/// See [`crate::move_to_trash`].
pub(crate) fn move_to_trash(path: &std::path::Path) -> CaptureResult<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };
    // A list of paths, each ending in a NUL, then one more NUL.
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` is double-NUL terminated and outlives the call.
    match unsafe { SHFileOperationW(&mut operation) } {
        0 if !operation.fAnyOperationsAborted.as_bool() => Ok(()),
        0 => Err(CaptureError::Platform(
            "moving to the Recycle Bin was cancelled".into(),
        )),
        code => Err(CaptureError::Platform(format!(
            "could not move to the Recycle Bin (error {code:#x})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_crops_with_row_padding() {
        // 2x2 BGRA with 4 bytes of padding per row.
        #[rustfmt::skip]
        let bgra = [
            1, 2, 3, 0,    4, 5, 6, 0,    9, 9, 9, 9,
            7, 8, 9, 0,   10, 11, 12, 0,  9, 9, 9, 9,
        ];
        assert_eq!(
            bgra_to_rgba(&bgra, 12, 1, 1, 1, 1),
            vec![12, 11, 10, 255],
            "bottom-right pixel, swizzled and opaque"
        );
        assert_eq!(bgra_to_rgba(&bgra, 12, 0, 0, 2, 2).len(), 16);
    }
}
