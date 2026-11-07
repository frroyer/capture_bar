//! The full-desktop overlay over a frozen image of the screens (M2.4, FR-9).

use cb_core::{window_at, CaptureMode, Rect, WindowId, WindowInfo};
use egui::{
    pos2, vec2, Align2, CentralPanel, Color32, ColorImage, Context, CursorIcon, FontFamily, FontId,
    Frame, Key, Painter, Pos2, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
    ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::palette;

const DIM: Color32 = Color32::from_black_alpha(120);

/// What the user decided, which ends the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayResult {
    Cancelled,
    Window(WindowId),
}

pub struct Overlay {
    mode: CaptureMode,
    desktop: cb_core::Frame,
    /// The desktop's top-left corner, in global physical pixels.
    origin: (i32, i32),
    windows: Vec<WindowInfo>,
    texture: Option<TextureHandle>,
    focused: bool,
}

/// A window position to global physical pixels.
fn to_global(pos: Pos2, ppp: f32, origin: (i32, i32)) -> (i32, i32) {
    (
        origin.0 + (pos.x * ppp).round() as i32,
        origin.1 + (pos.y * ppp).round() as i32,
    )
}

/// A global rectangle to window points.
fn to_local(rect: Rect, ppp: f32, origin: (i32, i32)) -> egui::Rect {
    egui::Rect::from_min_size(
        pos2((rect.x - origin.0) as f32, (rect.y - origin.1) as f32) / ppp,
        vec2(rect.width as f32, rect.height as f32) / ppp,
    )
}

/// Darken everything but `hole`.
fn paint_dim(painter: &Painter, screen: egui::Rect, hole: Option<egui::Rect>) {
    let Some(hole) = hole
        .map(|h| h.intersect(screen))
        .filter(|h| h.is_positive())
    else {
        painter.rect_filled(screen, 0.0, DIM);
        return;
    };
    let (s, h) = (screen, hole);
    for band in [
        egui::Rect::from_min_max(s.min, pos2(s.max.x, h.min.y)),
        egui::Rect::from_min_max(pos2(s.min.x, h.max.y), s.max),
        egui::Rect::from_min_max(pos2(s.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        egui::Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(s.max.x, h.max.y)),
    ] {
        painter.rect_filled(band, 0.0, DIM);
    }
}

/// Text on a dark chip, its `anchor` point at `pos`, kept inside `screen`.
fn paint_label(painter: &Painter, screen: egui::Rect, pos: Pos2, anchor: Align2, text: &str) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::new(13.0, FontFamily::Proportional),
        palette::TEXT,
    );
    let size = galley.size() + vec2(16.0, 8.0);
    let rect = anchor.anchor_size(pos, size);
    let rect = rect.translate(vec2(
        (screen.min.x - rect.min.x).max(0.0) + (screen.max.x - rect.max.x).min(0.0),
        (screen.min.y - rect.min.y).max(0.0) + (screen.max.y - rect.max.y).min(0.0),
    ));
    painter.rect_filled(rect, 6.0, palette::BAR);
    painter.galley(rect.min + vec2(8.0, 4.0), galley, palette::TEXT);
}

impl Overlay {
    pub fn new(
        mode: CaptureMode,
        desktop: cb_core::Frame,
        origin: (i32, i32),
        windows: Vec<WindowInfo>,
    ) -> Self {
        Self {
            mode,
            desktop,
            origin,
            windows,
            texture: None,
            focused: false,
        }
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// Draw it in its own OS window. Call each frame until it returns a result.
    pub fn show(&mut self, ctx: &Context) -> Option<OverlayResult> {
        let ppp = ctx.pixels_per_point();
        let builder = ViewportBuilder::default()
            .with_title("Capture Bar selection")
            .with_position(pos2(self.origin.0 as f32, self.origin.1 as f32) / ppp)
            .with_inner_size(vec2(self.desktop.width as f32, self.desktop.height as f32) / ppp)
            .with_decorations(false)
            .with_always_on_top()
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(true);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("capture overlay"),
            builder,
            |ui, _class| self.ui(ui),
        )
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<OverlayResult> {
        let ctx = ui.ctx().clone();
        if !self.focused {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        let desktop = &self.desktop;
        let texture = self
            .texture
            .get_or_insert_with(|| {
                let size = [desktop.width as usize, desktop.height as usize];
                let image = ColorImage::from_rgba_unmultiplied(size, &desktop.rgba);
                ctx.load_texture("frozen desktop", image, TextureOptions::NEAREST)
            })
            .clone();

        let mut result = None;
        CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| result = self.draw(ui, &texture));
        result
    }

    fn draw(&mut self, ui: &mut Ui, texture: &TextureHandle) -> Option<OverlayResult> {
        let ppp = ui.ctx().pixels_per_point();
        let screen = ui.max_rect();
        // One image pixel per screen pixel: an `Image` widget would read pixels as points.
        let whole = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter()
            .image(texture.id(), screen, whole, Color32::WHITE);
        let response = ui.interact(screen, ui.scope_id().with("overlay"), Sense::click());
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);

        let hovered = response
            .hover_pos()
            .and_then(|pos| window_at(to_global(pos, ppp, self.origin), &self.windows));
        let highlight = hovered.map(|w| to_local(w.bounds, ppp, self.origin));

        let painter = ui.painter();
        paint_dim(painter, screen, highlight);
        if let (Some(window), Some(rect)) = (hovered, highlight) {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(2.0, palette::ACCENT),
                StrokeKind::Inside,
            );
            let at = rect.left_top() + vec2(8.0, 8.0);
            paint_label(painter, screen, at, Align2::LEFT_TOP, &window.title);
        }
        let hint = "Click a window to capture it  ·  Esc cancels";
        let top = screen.center_top() + vec2(0.0, 16.0);
        paint_label(painter, screen, top, Align2::CENTER_TOP, hint);

        if ui.input(|i| i.key_pressed(Key::Escape)) || response.secondary_clicked() {
            return Some(OverlayResult::Cancelled);
        }
        if response.clicked() {
            return hovered.map(|w| OverlayResult::Window(w.id));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_and_local_agree_at_fractional_scales_and_negative_origins() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            for origin in [(0, 0), (-1920, -200)] {
                let rect = Rect {
                    x: origin.0 + 300,
                    y: origin.1 + 40,
                    width: 800,
                    height: 600,
                };
                let local = to_local(rect, ppp, origin);
                assert_eq!(to_global(local.min, ppp, origin), (rect.x, rect.y));
                assert_eq!(
                    to_global(local.max, ppp, origin),
                    (rect.x + 800, rect.y + 600)
                );
            }
        }
    }
}
