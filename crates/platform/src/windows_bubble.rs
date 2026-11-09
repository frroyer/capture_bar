//! Draws the webcam bubble into recorded frames on the GPU (US-5.4): the centre square
//! of the camera picture, mirrored as in the bar's preview, cut to a disc with a soft
//! edge and a white rim, blended onto the frame where the bubble is on screen.

use crate::windows_camera::CamFrame;
use windows::core::{s, Result as WinResult, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

const SHADER: &str = r"
Texture2D cam : register(t0);
SamplerState linear_clamp : register(s0);
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
V vs(uint id : SV_VertexID) {
    V v;
    float2 t = float2(id & 1, id >> 1);
    v.pos = float4(t.x * 2 - 1, 1 - t.y * 2, 0, 1);
    v.uv = t;
    return v;
}
float4 ps(V v) : SV_Target {
    float r = length(v.uv - 0.5) * 2;   // 1 on the edge
    float aa = fwidth(r);
    float alpha = 1 - smoothstep(1 - aa, 1, r);
    float rim = smoothstep(0.955 - aa, 0.955, r);
    float3 c = cam.Sample(linear_clamp, float2(1 - v.uv.x, v.uv.y)).rgb;  // mirrored
    return float4(lerp(c, float3(1, 1, 1), rim), alpha);
}
";

pub(crate) struct BubblePainter {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    /// The camera's centre square, its side and the picture it holds.
    picture: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, u32, u64)>,
}

fn compile(entry: PCSTR, target: PCSTR) -> WinResult<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source outlives the call; the out-pointers are valid.
    let compiled = unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            s!("bubble"),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let (Err(e), Some(errors)) = (&compiled, errors) {
        // SAFETY: the blob holds the compiler's message, as bytes.
        let text = unsafe {
            std::slice::from_raw_parts(
                errors.GetBufferPointer() as *const u8,
                errors.GetBufferSize(),
            )
        };
        tracing::error!("bubble shader: {e}: {}", String::from_utf8_lossy(text));
    }
    compiled?;
    Ok(code.expect("D3DCompile succeeded without code"))
}

fn bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: the blob owns this many bytes for its lifetime.
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
}

impl BubblePainter {
    pub fn new(device: &ID3D11Device) -> WinResult<Self> {
        let (vs_code, ps_code) = (
            compile(s!("vs"), s!("vs_4_0"))?,
            compile(s!("ps"), s!("ps_4_0"))?,
        );
        let (mut vs, mut ps, mut sampler, mut blend) = (None, None, None, None);
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_SRC_ALPHA,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ZERO,
            DestBlendAlpha: D3D11_BLEND_ONE,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        // SAFETY: descriptors and bytecode are valid for each call.
        unsafe {
            device.CreateVertexShader(bytes(&vs_code), None, Some(&mut vs))?;
            device.CreatePixelShader(bytes(&ps_code), None, Some(&mut ps))?;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
            device.CreateBlendState(&blend_desc, Some(&mut blend))?;
        }
        Ok(Self {
            vs: vs.expect("vertex shader"),
            ps: ps.expect("pixel shader"),
            sampler: sampler.expect("sampler"),
            blend: blend.expect("blend state"),
            picture: None,
        })
    }

    /// Upload the centre square of `frame`, unless it is the picture already there.
    fn upload(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        frame: &CamFrame,
    ) -> WinResult<()> {
        let side = frame.width.min(frame.height);
        if self.picture.as_ref().is_some_and(|p| p.3 == frame.seq) {
            return Ok(());
        }
        if self.picture.as_ref().is_none_or(|p| p.2 != side) {
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
                Usage: D3D11_USAGE_DYNAMIC,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
            };
            let (mut texture, mut view) = (None, None);
            // SAFETY: `desc` and the out-pointers are valid for each call.
            unsafe {
                device.CreateTexture2D(&desc, None, Some(&mut texture))?;
                let texture = texture.expect("texture");
                device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
                self.picture = Some((texture, view.expect("view"), side, u64::MAX));
            }
        }
        let picture = self.picture.as_mut().expect("made above");
        let (left, top) = ((frame.width - side) / 2, (frame.height - side) / 2);
        let row = side as usize * 4;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the mapping holds `side` rows of `RowPitch` bytes until Unmap; each
        // source row lies inside `frame.bgra`.
        unsafe {
            context.Map(&picture.0, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            for y in 0..side as usize {
                let from = ((top as usize + y) * frame.width as usize + left as usize) * 4;
                std::ptr::copy_nonoverlapping(
                    frame.bgra[from..from + row].as_ptr(),
                    (mapped.pData as *mut u8).add(y * mapped.RowPitch as usize),
                    row,
                );
            }
            context.Unmap(&picture.0, 0);
        }
        picture.3 = frame.seq;
        Ok(())
    }

    /// Draw `frame` as a disc filling `(x, y, size, size)` of `target`, which may
    /// reach past its edges.
    pub fn paint(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        target: &ID3D11Texture2D,
        (x, y, size): (i32, i32, u32),
        frame: &CamFrame,
    ) -> WinResult<()> {
        self.upload(device, context, frame)?;
        let view = self.picture.as_ref().expect("uploaded").1.clone();
        let mut rtv = None;
        let viewport = D3D11_VIEWPORT {
            TopLeftX: x as f32,
            TopLeftY: y as f32,
            Width: size as f32,
            Height: size as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: every bound object lives through the draw; bindings are cleared after,
        // so the frame texture is free for the encoder.
        unsafe {
            device.CreateRenderTargetView(target, None, Some(&mut rtv))?;
            context.OMSetRenderTargets(Some(&[rtv]), None);
            context.RSSetViewports(Some(&[viewport]));
            context.IASetInputLayout(None);
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.VSSetShader(&self.vs, None);
            context.PSSetShader(&self.ps, None);
            context.PSSetShaderResources(0, Some(&[Some(view)]));
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.OMSetBlendState(&self.blend, Some(&[0.0; 4]), u32::MAX);
            context.Draw(4, 0);
            context.OMSetRenderTargets(None, None);
            context.PSSetShaderResources(0, Some(&[None]));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BGRA of `texture` (`side` x `side`), read back through a staging copy.
    fn read_back(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        texture: &ID3D11Texture2D,
        side: u32,
    ) -> Vec<u8> {
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
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: valid descriptors; the mapping is read before Unmap.
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .unwrap();
            let staging = staging.unwrap();
            context.CopyResource(&staging, texture);
            context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .unwrap();
            let mut out = Vec::new();
            for y in 0..side as usize {
                let row = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                out.extend_from_slice(std::slice::from_raw_parts(row, side as usize * 4));
            }
            context.Unmap(&staging, 0);
            out
        }
    }

    /// US-5.4: the camera lands as a mirrored disc where the bubble is, and nowhere else.
    #[test]
    fn bubble_is_a_mirrored_disc_at_its_place() {
        let Ok(gpu) = crate::windows::shared_gpu() else {
            eprintln!("skipped: no hardware D3D11 device");
            return;
        };
        let device = &gpu.device;
        // SAFETY: no preconditions.
        let context = &unsafe { device.GetImmediateContext() }.unwrap();
        // A black frame, as the recording's copy of the screen.
        let side = 200u32;
        let black = vec![0u8; (side * side * 4) as usize];
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
            pSysMem: black.as_ptr().cast(),
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
        // A 4:3 camera picture: red on the left, blue on the right (BGRA).
        let (w, h) = (64u32, 48u32);
        let bgra = (0..w * h)
            .flat_map(|i| {
                if i % w < w / 2 {
                    [0, 0, 255, 255]
                } else {
                    [255, 0, 0, 255]
                }
            })
            .collect();
        let picture = CamFrame {
            width: w,
            height: h,
            bgra,
            seq: 1,
        };
        let mut painter = BubblePainter::new(device).unwrap();
        painter
            .paint(device, context, &frame, (50, 50, 100), &picture)
            .unwrap();
        let pixels = read_back(device, context, &frame, side);
        let at = |x: u32, y: u32| {
            let i = ((y * side + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        assert_eq!(at(10, 10), [0, 0, 0], "outside the bubble");
        assert_eq!(
            at(52, 52),
            [0, 0, 0],
            "a corner of its square, outside the disc"
        );
        let [b, _, r] = at(75, 100);
        assert!(
            b > 200 && r < 50,
            "mirrored: blue on the left, got {:?}",
            at(75, 100)
        );
        let [b, _, r] = at(125, 100);
        assert!(
            r > 200 && b < 50,
            "mirrored: red on the right, got {:?}",
            at(125, 100)
        );
        let edge = at(100, 52);
        assert!(edge.iter().all(|&c| c > 150), "white rim, got {edge:?}");
    }
}
