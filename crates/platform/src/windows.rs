//! Windows backend.
//! Capture: Windows.Graphics.Capture (one session per grab, polled free-threaded).
//! Encode:  Media Foundation H.264, see `windows_spike` (US-1.2).
//! Exclude the bar from captures: SetWindowDisplayAffinity (US-2.1).

use crate::stub::StubScreenSource;
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
    DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SetWindowDisplayAffinity, SetWindowPos, SM_SWAPBUTTON, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WDA_EXCLUDEFROMCAPTURE, WM_NCCALCSIZE,
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

/// winit keeps a 1 px strip above the client area of borderless windows, which
/// shows wherever the bar's rounded corners leave the window transparent. Answering
/// the message here makes the client area cover the whole window.
unsafe extern "system" fn fill_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NCCALCSIZE && wparam.0 != 0 {
        return LRESULT(0);
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
struct SharedGpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
}

// SAFETY: the device is multithread-protected below, which also serialises the
// immediate context; the WinRT wrapper is free-threaded.
unsafe impl Send for SharedGpu {}
unsafe impl Sync for SharedGpu {}

// chisle: no device-lost recovery (driver reset breaks grabs until relaunch); add in US-4.4.
fn shared_gpu() -> WinResult<&'static SharedGpu> {
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

fn capture_item(monitor: HMONITOR) -> WinResult<GraphicsCaptureItem> {
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

/// A running capture session on one monitor, with a CPU-readable copy of the newest frame.
pub(crate) struct MonitorCapture {
    pub device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    staging: ID3D11Texture2D,
    pub width: u32,
    pub height: u32,
}

impl MonitorCapture {
    pub fn start(monitor: HMONITOR) -> WinResult<Self> {
        let started = Self::start_with(&capture_item(monitor)?);
        if started.is_err() {
            // The cached item may be stale (monitor unplugged): rebuild it on the next grab.
            forget_capture_item(monitor);
        }
        started
    }

    fn start_with(item: &GraphicsCaptureItem) -> WinResult<Self> {
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
        let mut latest = None;
        // TryGetNextFrame fails or returns null once the pool is empty.
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            latest = Some(frame);
        }
        let Some(frame) = latest else {
            return Ok(false);
        };
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: the surface is a D3D11 texture on our device, same size and format as staging.
        unsafe {
            let texture: ID3D11Texture2D = access.GetInterface()?;
            self.context.CopyResource(&self.staging, &texture);
        }
        Ok(true)
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

pub struct WinScreenSource {
    inner: StubScreenSource,
}

impl WinScreenSource {
    pub fn new() -> Self {
        Self {
            inner: StubScreenSource,
        }
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
        // US-4.3
        self.inner.windows()
    }

    fn grab(&self, target: &CaptureTarget) -> CaptureResult<Frame> {
        let (monitor, region) = match target {
            CaptureTarget::Display(id) => (resolve_display(*id)?, None),
            CaptureTarget::Region(rect) => {
                let r = RECT {
                    left: rect.x,
                    top: rect.y,
                    right: rect.x + rect.width as i32,
                    bottom: rect.y + rect.height as i32,
                };
                // SAFETY: `r` is valid for the duration of the call.
                let m = unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST) };
                (m, Some(*rect))
            }
            CaptureTarget::Window(_) => return self.inner.grab(target), // US-4.3
        };

        let capture = MonitorCapture::start(monitor).map_err(platform_err)?;
        capture.wait_first_frame()?;
        let captured_at = SystemTime::now();

        // Crop to the part of the region on this monitor, in monitor coordinates.
        let (x, y, w, h) = match region {
            None => (0, 0, capture.width, capture.height),
            Some(rect) => {
                let (origin, ..) = monitor_bounds(monitor).ok_or(CaptureError::TargetGone)?;
                let left = (rect.x - origin.left).max(0) as u32;
                let top = (rect.y - origin.top).max(0) as u32;
                let right =
                    ((rect.x - origin.left + rect.width as i32).max(0) as u32).min(capture.width);
                let bottom =
                    ((rect.y - origin.top + rect.height as i32).max(0) as u32).min(capture.height);
                if right <= left || bottom <= top {
                    return Err(CaptureError::Platform("region is off screen".into()));
                }
                (left, top, right - left, bottom - top)
            }
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

    fn start_stream(
        &self,
        target: &CaptureTarget,
        fps: u32,
        sink: Box<dyn FrameSink>,
    ) -> CaptureResult<Box<dyn StreamHandle>> {
        // US-5.1; the US-1.2 spike records through `windows_spike` directly.
        self.inner.start_stream(target, fps, sink)
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
