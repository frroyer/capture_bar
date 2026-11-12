//! Quick Look on the buffer: a window with one capture at full size, to zoom into and
//! step through the others without leaving the tray.

use std::sync::Arc;

use cb_buffer::ItemId;
use egui::{
    vec2, CentralPanel, Context, CursorIcon, Frame, Key, Margin, Panel, Pos2, Rect, Sense,
    TextureFilter, TextureHandle, TextureOptions, Ui, Vec2, ViewportBuilder, ViewportCommand,
    ViewportId,
};

use crate::tr;
use crate::{chip, palette, ChipStyle, Piece};

const FOOTER_HEIGHT: f32 = 44.0;
/// The most a picture is blown up, in screen pixels per image pixel.
const MAX_ZOOM: f32 = 32.0;

/// What the user asked of the Quick Look window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekAction {
    Close,
    /// The newer capture.
    Previous,
    /// The older capture.
    Next,
    Annotate,
    /// Open a recording in the system's video player.
    Play,
}

pub struct Peek {
    item: ItemId,
    title: String,
    frame: Option<Arc<cb_core::Frame>>,
    /// `frame` is not in `texture` yet.
    fresh: bool,
    /// A recording: its frames come one after another and fill the window.
    video: bool,
    texture: Option<TextureHandle>,
    /// Points per image pixel; `None` fits the picture in the window.
    zoom: Option<f32>,
    /// From the window's centre to the picture's, in points.
    offset: Vec2,
    focused: bool,
}

impl Peek {
    pub fn new(item: ItemId, title: impl Into<String>, video: bool) -> Self {
        Self {
            item,
            title: title.into(),
            frame: None,
            fresh: false,
            video,
            texture: None,
            zoom: None,
            offset: Vec2::ZERO,
            focused: false,
        }
    }

    /// The capture shown.
    pub fn item(&self) -> ItemId {
        self.item
    }

    /// Show another capture, fitted, until its image is set.
    pub fn show_item(&mut self, item: ItemId, title: impl Into<String>, video: bool) {
        *self = Self {
            focused: self.focused,
            ..Self::new(item, title, video)
        };
    }

    /// The full image of `item`, once read, or a recording's next frame. Ignored if
    /// another capture is shown by now.
    pub fn set_frame(&mut self, item: ItemId, frame: Arc<cb_core::Frame>) {
        if item == self.item {
            self.frame = Some(frame);
            self.fresh = true;
        }
    }

    /// Draw it in its own OS window. `thumbnail` stands in until the image is read;
    /// `position` is the capture's place in the buffer, from 1, and the count.
    pub fn show(
        &mut self,
        ctx: &Context,
        thumbnail: Option<&TextureHandle>,
        position: (usize, usize),
    ) -> Option<PeekAction> {
        let monitor = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(vec2(1280.0, 800.0));
        let builder = ViewportBuilder::default()
            .with_title(&self.title)
            .with_inner_size(monitor * 0.75)
            .with_min_inner_size(vec2(360.0, 240.0))
            .with_active(true);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("peek"), builder, |ui, _| {
            self.ui(ui, thumbnail, position)
        })
    }

    fn ui(
        &mut self,
        ui: &mut Ui,
        thumbnail: Option<&TextureHandle>,
        position: (usize, usize),
    ) -> Option<PeekAction> {
        let ctx = ui.ctx().clone();
        if !self.focused {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            return Some(PeekAction::Close);
        }
        if let (true, Some(frame)) = (std::mem::take(&mut self.fresh), &self.frame) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            // Sharp pixels when zoomed in, smooth text when fitted.
            let options = TextureOptions {
                magnification: TextureFilter::Nearest,
                mipmap_mode: Some(TextureFilter::Linear),
                ..TextureOptions::LINEAR
            };
            match &mut self.texture {
                Some(texture) => texture.set(image, options),
                None => self.texture = Some(ctx.load_texture("peek", image, options)),
            }
        }
        let mut action = None;
        Panel::bottom("peek footer")
            .exact_size(FOOTER_HEIGHT)
            .frame(
                Frame::new()
                    .fill(palette::BAR)
                    .inner_margin(Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| action = self.footer(ui, position));
        CentralPanel::default()
            .frame(Frame::new().fill(palette::SLOT_BG))
            .show(ui, |ui| self.canvas(ui, thumbnail));
        action.or_else(|| keys(ui))
    }

    fn footer(&self, ui: &mut Ui, (index, count): (usize, usize)) -> Option<PeekAction> {
        let style = ChipStyle {
            fill: palette::CHIP,
            color: palette::TEXT,
            height: 28.0,
        };
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
            let mut button = |ui: &mut Ui, enabled: bool, text: &str, tip: String, what| {
                let clicked = ui
                    .add_enabled_ui(enabled, |ui| {
                        chip(ui, &[Piece::Text(text)], &style, &tip).clicked()
                    })
                    .inner;
                if clicked {
                    action = Some(what);
                }
            };
            button(
                ui,
                index > 1,
                "‹",
                tr!("peek-previous"),
                PeekAction::Previous,
            );
            ui.label(egui::RichText::new(format!("{index} / {count}")).color(palette::TEXT));
            button(ui, index < count, "›", tr!("peek-next"), PeekAction::Next);
            if let (Some(zoom), Some(_)) = (self.zoom, &self.texture) {
                let percent = (zoom * ui.ctx().pixels_per_point() * 100.0).round();
                ui.label(egui::RichText::new(format!("{percent}%")).color(palette::MUTED));
            }
            ui.label(egui::RichText::new(tr!("peek-hint")).color(palette::MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, what) = if self.video {
                    (tr!("card-play"), PeekAction::Play)
                } else {
                    (tr!("card-annotate"), PeekAction::Annotate)
                };
                button(ui, true, &label, label.clone(), what);
            });
        });
        action
    }

    fn canvas(&mut self, ui: &mut Ui, thumbnail: Option<&TextureHandle>) {
        let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let Some(texture) = self.texture.as_ref().or(thumbnail).cloned() else {
            return;
        };
        let full = self.texture.is_some();
        let image = texture.size_vec2();
        let ppp = ui.ctx().pixels_per_point();
        let fits = (area.width() / image.x).min(area.height() / image.y);
        // A small capture is not blown up to fit; the thumbnail is, until the image is read.
        // A recording's frames are scaled down already: they fill the window too.
        let fit = if full && !self.video {
            fits.min(1.0 / ppp)
        } else {
            fits
        };
        if full {
            let pointer = response.hover_pos();
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll / 200.0).exp();
            if let (Some(at), true) = (pointer, factor != 1.0) {
                let scale = self.zoom.unwrap_or(fit);
                let new = (scale * factor).clamp(fit.min(1.0 / ppp), MAX_ZOOM / ppp);
                self.zoom_at(area, image, scale, new, at);
            }
            if response.double_clicked() {
                let scale = self.zoom.unwrap_or(fit);
                match (self.zoom, pointer) {
                    (None, Some(at)) => {
                        self.zoom_at(area, image, scale, (1.0 / ppp).max(fit * 2.0), at)
                    }
                    _ => self.zoom = None,
                }
            }
            if ui.input(|i| i.key_pressed(Key::Num0)) {
                self.zoom = None;
            }
            if ui.input(|i| i.key_pressed(Key::Num1)) {
                self.zoom = Some(1.0 / ppp);
            }
            if self.zoom.is_some() {
                self.offset += response.drag_delta();
                if response.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                } else if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grab);
                }
            }
        }
        if self.zoom.is_none() {
            self.offset = Vec2::ZERO;
        }
        let scale = self.zoom.unwrap_or(fit);
        let shown = image * scale;
        // Kept over the window: no panning past the picture's edges.
        let room = ((shown - area.size()) / 2.0).max(Vec2::ZERO);
        self.offset = self.offset.clamp(-room, room);
        let rect = Rect::from_center_size(area.center() + self.offset, shown);
        ui.painter().with_clip_rect(area).image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    /// Zoom from `scale` to `new` points per pixel, keeping the image pixel under `at`.
    fn zoom_at(&mut self, area: Rect, image: Vec2, scale: f32, new: f32, at: Pos2) {
        let origin = area.center() + self.offset - image * scale / 2.0;
        let under = (at - origin) / scale;
        let new_origin = at - under * new;
        self.offset = new_origin + image * new / 2.0 - area.center();
        self.zoom = Some(new);
    }
}

fn keys(ui: &Ui) -> Option<PeekAction> {
    ui.input(|i| {
        if !i.modifiers.is_none() {
            None
        } else if i.key_pressed(Key::Escape) || i.key_pressed(Key::Space) {
            Some(PeekAction::Close)
        } else if i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp) {
            Some(PeekAction::Previous)
        } else if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown) {
            Some(PeekAction::Next)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_keeps_the_pixel_under_the_cursor() {
        let mut peek = Peek::new(ItemId(1), "", false);
        let area = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0));
        let image = vec2(800.0, 600.0);
        let at = Pos2::new(100.0, 50.0);
        let origin = |p: &Peek, scale: f32| area.center() + p.offset - image * scale / 2.0;
        let before = (at - origin(&peek, 0.5)) / 0.5;
        peek.zoom_at(area, image, 0.5, 2.0, at);
        let after = (at - origin(&peek, 2.0)) / 2.0;
        assert!((before - after).length() < 1e-3, "{before:?} → {after:?}");
        assert_eq!(peek.zoom, Some(2.0));
    }
}
