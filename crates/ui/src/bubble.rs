//! The webcam bubble (US-5.4, M1): a round, mirrored camera picture in its own small
//! window, dragged anywhere on screen. Recordings draw the camera where it is, so it
//! shows in the video without the window itself being captured.

use crate::place::{keep_at, placed};
use crate::{palette, tr};
use egui::{
    epaint::{Mesh, Vertex},
    pos2, vec2, CentralPanel, Color32, ColorImage, Context, CursorIcon, FontFamily, FontId, Frame,
    Pos2, Sense, Stroke, TextureHandle, TextureOptions, ViewportBuilder, ViewportCommand,
    ViewportId,
};

/// Diameter, in points.
pub const BUBBLE_SIZE: f32 = 180.0;

#[derive(Default)]
pub struct CameraBubble {
    texture: Option<TextureHandle>,
    /// The camera picture's width over its height.
    aspect: f32,
    /// Its window is in place and showing.
    shown: bool,
}

impl CameraBubble {
    /// Show a new camera picture from now on.
    pub fn set_picture(&mut self, ctx: &Context, frame: &cb_core::Frame) {
        let size = [frame.width as usize, frame.height as usize];
        let image = ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
        self.aspect = frame.width as f32 / frame.height.max(1) as f32;
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("camera", image, TextureOptions::LINEAR)),
        }
    }

    /// Draw the bubble for one frame, first at `start` (global physical pixels, a
    /// square of `BUBBLE_SIZE` points at its monitor's scale); the user moves it from
    /// there. It stays hidden until `dressed` (by the caller, once the window exists)
    /// and in place. Returns where it is now, as a square in global physical pixels,
    /// once its window is open.
    pub fn show(
        &mut self,
        ctx: &Context,
        start: cb_core::Rect,
        dressed: bool,
    ) -> Option<cb_core::Rect> {
        let builder = ViewportBuilder::default()
            .with_title(tr!("bubble-title"))
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false);
        let builder = placed(builder, ctx, start, self.shown);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("camera bubble"),
            builder,
            |ui, _class| {
                // Placed once; after that it goes where the user drags it.
                if !self.shown {
                    self.shown = keep_at(ui.ctx(), start) && dressed;
                }
                CentralPanel::default()
                    .frame(Frame::NONE)
                    .show(ui, |ui| self.draw(ui));
                let inner = ui.ctx().input(|i| i.viewport().inner_rect)?;
                let ppp = ui.ctx().pixels_per_point();
                self.shown.then_some(cb_core::Rect {
                    x: (inner.min.x * ppp).round() as i32,
                    y: (inner.min.y * ppp).round() as i32,
                    width: (inner.width() * ppp).round() as u32,
                    height: (inner.height() * ppp).round() as u32,
                })
            },
        )
    }

    fn draw(&self, ui: &mut egui::Ui) {
        let area = ui.max_rect();
        let response = ui.interact(
            area,
            ui.make_persistent_id("camera bubble drag"),
            Sense::drag(),
        );
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        let center = area.center();
        let radius = area.width().min(area.height()) / 2.0 - 1.0;
        let painter = ui.painter();
        if let Some(texture) = &self.texture {
            painter.add(disc(texture, center, radius, self.aspect));
        } else {
            painter.circle_filled(center, radius, palette::BAR);
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                tr!("bubble-starting"),
                FontId::new(13.0, FontFamily::Proportional),
                palette::MUTED,
            );
        }
        painter.circle_stroke(center, radius - 2.0, Stroke::new(4.0, Color32::WHITE));
    }
}

/// The centre square of the picture, mirrored, as a disc: a triangle fan.
fn disc(texture: &TextureHandle, center: Pos2, radius: f32, aspect: f32) -> egui::Shape {
    // The part of the picture's width (or height) the square takes.
    let (span_x, span_y) = if aspect >= 1.0 {
        (1.0 / aspect, 1.0)
    } else {
        (1.0, aspect)
    };
    let uv = |dx: f32, dy: f32| pos2(0.5 - dx * span_x / 2.0, 0.5 + dy * span_y / 2.0);
    let mut mesh = Mesh::with_texture(texture.id());
    mesh.vertices.push(Vertex {
        pos: center,
        uv: uv(0.0, 0.0),
        color: Color32::WHITE,
    });
    const STEPS: u32 = 96;
    for i in 0..=STEPS {
        let angle = std::f32::consts::TAU * i as f32 / STEPS as f32;
        let (dx, dy) = (angle.cos(), angle.sin());
        mesh.vertices.push(Vertex {
            pos: center + vec2(dx, dy) * radius,
            uv: uv(dx, dy),
            color: Color32::WHITE,
        });
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    egui::Shape::mesh(mesh)
}
