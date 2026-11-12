//! The webcam bubble (US-5.4, M1): a mirrored camera picture in its own small window,
//! round or a rounded square or portrait (Settings), dragged anywhere on screen. Recordings draw the camera where it is, so it
//! shows in the video without the window itself being captured.

use crate::place::{keep_at, placed};
use crate::{palette, tr};
use cb_core::BubbleShape;
use egui::{
    pos2, vec2, CentralPanel, Color32, ColorImage, Context, CornerRadius, CursorIcon, FontFamily,
    FontId, Frame, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, ViewportBuilder,
    ViewportCommand, ViewportId,
};

/// Height, in points; the width follows the shape.
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

    /// Draw the bubble for one frame, first at `start` (global physical pixels,
    /// `BUBBLE_SIZE` points high at its monitor's scale); the user moves it from there.
    /// It stays hidden until `dressed` (by the caller, once the window exists) and in
    /// place, and takes `shape`'s proportions when they change. Returns where it is now,
    /// in global physical pixels, once its window is open.
    pub fn show(
        &mut self,
        ctx: &Context,
        start: cb_core::Rect,
        dressed: bool,
        shape: BubbleShape,
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
                    .show(ui, |ui| self.draw(ui, shape));
                let inner = ui.ctx().input(|i| i.viewport().inner_rect)?;
                if self.shown {
                    reshape(ui.ctx(), inner, shape.aspect());
                }
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

    fn draw(&self, ui: &mut egui::Ui, shape: BubbleShape) {
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
            // Ctrl + scroll (or a pinch) resizes it.
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom != 1.0 {
                resize(ui.ctx(), zoom);
            }
        }
        let rect = area.shrink(1.0);
        let corner = (shape.corner() * rect.width().min(rect.height())).min(255.0) as u8;
        let rim = Stroke::new(4.0, Color32::WHITE);
        let painter = ui.painter();
        let picture = egui::epaint::RectShape::new(
            rect,
            CornerRadius::same(corner),
            if self.texture.is_some() {
                Color32::WHITE
            } else {
                palette::BAR
            },
            rim,
            StrokeKind::Inside,
        );
        match &self.texture {
            Some(texture) => {
                // The rim on its own shape: epaint cannot stroke a textured rect (its
                // stroke would go into the textured mesh, a debug assert).
                let uv = mirrored_cover(self.aspect, rect.aspect_ratio());
                let mut fill = picture.clone().with_texture(texture.id(), uv);
                fill.stroke = Stroke::NONE;
                painter.add(fill);
                let mut rim = picture;
                rim.fill = Color32::TRANSPARENT;
                painter.add(rim);
            }
            None => {
                painter.add(picture);
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    tr!("bubble-starting"),
                    FontId::new(13.0, FontFamily::Proportional),
                    palette::MUTED,
                );
            }
        }
    }
}

/// The part of a picture `picture` wide over high that fills a box `target` wide over
/// high, centred and mirrored, as texture coordinates.
fn mirrored_cover(picture: f32, target: f32) -> Rect {
    let (span_x, span_y) = if picture >= target {
        (target / picture, 1.0)
    } else {
        (1.0, picture / target)
    };
    // Left runs past right: the picture is mirrored, as people expect to see themselves.
    Rect::from_min_max(
        pos2(0.5 + span_x / 2.0, 0.5 - span_y / 2.0),
        pos2(0.5 - span_x / 2.0, 0.5 + span_y / 2.0),
    )
}

/// Give the window `aspect` (width over height) around its centre, keeping its height,
/// when the shape was changed or it opened at an older shape's size.
fn reshape(ctx: &Context, inner: Rect, aspect: f32) {
    let width = (inner.height() * aspect).round();
    if (width - inner.width()).abs() < 1.0 {
        return;
    }
    let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else {
        return;
    };
    let shift = (width - inner.width()) / 2.0;
    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min - vec2(shift, 0.0)));
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(width, inner.height())));
}

/// Smallest and largest height the user can resize it to, in points.
const MIN_SIZE: f32 = 80.0;
const MAX_SIZE: f32 = 480.0;

/// Scale the bubble's window by `zoom` around its centre, within the size limits.
fn resize(ctx: &Context, zoom: f32) {
    let Some((outer, inner)) = ctx.input(|i| {
        let v = i.viewport();
        v.outer_rect.zip(v.inner_rect)
    }) else {
        return;
    };
    let height = (inner.height() * zoom).clamp(MIN_SIZE, MAX_SIZE);
    let size = inner.size() * (height / inner.height());
    let grow = (size - inner.size()) / 2.0;
    if grow.y == 0.0 {
        return;
    }
    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min - grow));
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_cut_to_the_box_and_mirrored() {
        let uv = mirrored_cover(4.0 / 3.0, 1.0);
        assert!(uv.min.x > uv.max.x, "mirrored: {uv:?}");
        assert!((uv.min.x - uv.max.x - 0.75).abs() < 1e-6, "a square of 4:3");
        assert_eq!((uv.min.y, uv.max.y), (0.0, 1.0));
        let uv = mirrored_cover(4.0 / 3.0, 0.75);
        assert!((uv.min.x - uv.max.x - 0.5625).abs() < 1e-6, "3:4 of 4:3");
        let uv = mirrored_cover(0.5, 1.0);
        assert!(
            (uv.max.y - uv.min.y - 0.5).abs() < 1e-6,
            "a tall picture loses height"
        );
    }
}
