//! Masks of a demo session (US-10.5, DR-20): parts of the screen and windows blurred in
//! every frame recorded, before the encoder and the stills see it, so no clear copy of
//! them is ever written. The blur is a 32 px box average (the frame's mip level 5),
//! spread back smoothly: no letter survives it.

use crate::windows_bubble::{bytes, compile};
use cb_core::{Mask, Rect, WindowId};
use std::sync::Mutex;
use windows::core::{s, Result as WinResult};
use windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

/// The masks applied to recordings now; the app sets them for a demo session.
static MASKS: Mutex<Vec<Mask>> = Mutex::new(Vec::new());

pub(crate) fn set(masks: Vec<Mask>) {
    *MASKS.lock().unwrap_or_else(|e| e.into_inner()) = masks;
}

pub(crate) fn current() -> Vec<Mask> {
    MASKS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The parts of a `size` frame whose top-left corner is `origin` on the desktop that
/// `masks` cover, clipped to it. `window` finds where a window is, if it shows.
// chisle: a window is blurred where it is, even when another window covers it.
pub(crate) fn rects(
    masks: &[Mask],
    window: impl Fn(WindowId) -> Option<Rect>,
    origin: (i32, i32),
    (width, height): (u32, u32),
) -> Vec<Rect> {
    masks
        .iter()
        .filter_map(|mask| match *mask {
            Mask::Area(rect) => Some(rect),
            Mask::Window(id) => window(id),
        })
        .filter_map(|r| {
            let left = (r.x - origin.0).max(0);
            let top = (r.y - origin.1).max(0);
            let right = (r.x - origin.0 + r.width as i32).min(width as i32);
            let bottom = (r.y - origin.1 + r.height as i32).min(height as i32);
            (right > left && bottom > top).then(|| Rect {
                x: left,
                y: top,
                width: (right - left) as u32,
                height: (bottom - top) as u32,
            })
        })
        .collect()
}

const SHADER: &str = r"
Texture2D screen : register(t0);
SamplerState linear_clamp : register(s0);
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
V vs(uint id : SV_VertexID) {
    V v;
    float2 t = float2(id & 1, id >> 1);
    v.pos = float4(t.x * 2 - 1, 1 - t.y * 2, 0, 1);
    v.uv = t;
    return v;
}
// area: x, y, width, height as shares of the frame; level.x: the mip level to show.
cbuffer Mask : register(b0) { float4 area; float4 level; };
float4 ps(V v) : SV_Target {
    float2 uv = area.xy + v.uv * area.zw;
    return float4(screen.SampleLevel(linear_clamp, uv, level.x).rgb, 1);
}
";

/// The blurriest level drawn: 2^5 = 32 px averaged into one.
const LEVEL: u32 = 5;

pub(crate) struct MaskPainter {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    constants: ID3D11Buffer,
    /// A copy of the frame with its mip chain, its view and its size.
    scratch: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, (u32, u32))>,
}

impl MaskPainter {
    pub fn new(device: &ID3D11Device) -> WinResult<Self> {
        let (vs_code, ps_code) = (
            compile(SHADER, s!("vs"), s!("vs_4_0"))?,
            compile(SHADER, s!("ps"), s!("ps_4_0"))?,
        );
        let (mut vs, mut ps, mut sampler, mut constants) = (None, None, None, None);
        let constants_desc = D3D11_BUFFER_DESC {
            ByteWidth: 32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        // SAFETY: descriptors and bytecode are valid for each call.
        unsafe {
            device.CreateVertexShader(bytes(&vs_code), None, Some(&mut vs))?;
            device.CreatePixelShader(bytes(&ps_code), None, Some(&mut ps))?;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
            device.CreateBuffer(&constants_desc, None, Some(&mut constants))?;
        }
        Ok(Self {
            vs: vs.expect("vertex shader"),
            ps: ps.expect("pixel shader"),
            sampler: sampler.expect("sampler"),
            constants: constants.expect("constant buffer"),
            scratch: None,
        })
    }

    /// Blur `rects` of `frame`, a `size` BGRA texture that can be drawn into.
    pub fn paint(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        frame: &ID3D11Texture2D,
        size: (u32, u32),
        rects: &[Rect],
    ) -> WinResult<()> {
        if rects.is_empty() {
            return Ok(());
        }
        // As many levels as the frame has, up to LEVEL.
        let levels = (32 - size.0.max(size.1).leading_zeros()).min(LEVEL + 1);
        if self.scratch.as_ref().is_none_or(|s| s.2 != size) {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: levels,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
            };
            let (mut texture, mut view) = (None, None);
            // SAFETY: `desc` and the out-pointers are valid for each call.
            unsafe {
                device.CreateTexture2D(&desc, None, Some(&mut texture))?;
                let texture = texture.expect("texture");
                device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
                self.scratch = Some((texture, view.expect("view"), size));
            }
        }
        let (scratch, view, _) = self.scratch.clone().expect("made above");
        let mut rtv = None;
        // SAFETY: both textures share the device, format and top-level size; bindings are
        // cleared after the draws, so the frame is free for the encoder.
        unsafe {
            context.CopySubresourceRegion(&scratch, 0, 0, 0, 0, frame, 0, None);
            context.GenerateMips(&view);
            device.CreateRenderTargetView(frame, None, Some(&mut rtv))?;
            context.OMSetRenderTargets(Some(&[rtv]), None);
            context.OMSetBlendState(None, None, u32::MAX);
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.VSSetShader(&self.vs, None);
            context.PSSetShader(&self.ps, None);
            context.PSSetShaderResources(0, Some(&[Some(view)]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
        }
        let (fw, fh) = (size.0 as f32, size.1 as f32);
        for rect in rects {
            let (x, y, w, h) = (
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
            );
            let constants: [f32; 8] = [
                x / fw,
                y / fh,
                w / fw,
                h / fh,
                (levels - 1) as f32,
                0.0,
                0.0,
                0.0,
            ];
            let viewport = D3D11_VIEWPORT {
                TopLeftX: x,
                TopLeftY: y,
                Width: w,
                Height: h,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: the buffer holds 32 bytes, written whole before Unmap.
            unsafe {
                context.Map(
                    &self.constants,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped),
                )?;
                std::ptr::copy_nonoverlapping(constants.as_ptr(), mapped.pData.cast(), 8);
                context.Unmap(&self.constants, 0);
                context.RSSetViewports(Some(&[viewport]));
                context.Draw(4, 0);
            }
        }
        // SAFETY: unbinding only.
        unsafe {
            context.OMSetRenderTargets(None, None);
            context.PSSetShaderResources(0, Some(&[None]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn masks_land_in_frame_pixels_clipped_and_follow_their_window() {
        let masks = [
            Mask::Area(rect(1100, 50, 200, 100)),
            Mask::Window(WindowId(7)),
            Mask::Window(WindowId(8)),
            Mask::Area(rect(5000, 0, 10, 10)),
        ];
        let window = |id: WindowId| (id.0 == 7).then(|| rect(900, 500, 400, 300));
        // A 1280 x 720 area whose corner is at (1000, 0) on the desktop.
        let got = rects(&masks, window, (1000, 0), (1280, 720));
        assert_eq!(
            got,
            [rect(100, 50, 200, 100), rect(0, 500, 300, 220)],
            "the window is cut at the frame's edges; a hidden one and an off-frame area are left out"
        );
    }

    /// The masked part comes out blurred, the rest untouched. Skipped without a GPU.
    #[test]
    fn a_masked_area_is_blurred_and_nothing_else() {
        let Ok(gpu) = crate::windows::shared_gpu() else {
            eprintln!("skipped: no hardware D3D11 device");
            return;
        };
        let _turn = crate::windows_bubble::tests::GPU
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let device = &gpu.device;
        // SAFETY: no preconditions.
        let context = &unsafe { device.GetImmediateContext() }.unwrap();
        let side = 128u32;
        // One-pixel black and white stripes: text-like detail.
        let stripes: Vec<u8> = (0..side * side)
            .flat_map(|i| if i % 2 == 0 { [0, 0, 0, 255] } else { [255; 4] })
            .collect();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: side,
            Height: side,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: stripes.as_ptr().cast(),
            SysMemPitch: side * 4,
            SysMemSlicePitch: 0,
        };
        let mut frame = None;
        // SAFETY: valid descriptors and initial data.
        unsafe {
            device
                .CreateTexture2D(&desc, Some(&data), Some(&mut frame))
                .unwrap();
        }
        let frame = frame.unwrap();
        let mut painter = MaskPainter::new(device).unwrap();
        painter
            .paint(
                device,
                context,
                &frame,
                (side, side),
                &[rect(32, 32, 64, 64)],
            )
            .unwrap();
        let pixels = crate::windows_bubble::tests::read_back(device, context, &frame, side);
        let at = |x: u32, y: u32| pixels[((y * side + x) * 4) as usize];
        assert_eq!((at(10, 10), at(11, 10)), (0, 255), "stripes left outside");
        for (x, y) in [(40, 40), (41, 40), (64, 64), (65, 90)] {
            let v = at(x, y);
            assert!(
                (100..=155).contains(&v),
                "blurred to grey at ({x}, {y}): {v}"
            );
        }
    }
}
