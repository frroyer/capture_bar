//! Placing the bar's other windows (overlay, countdown, camera bubble) in global
//! physical pixels, on monitors whose scales differ.
//!
//! egui turns a viewport's points into pixels with that window's own scale, known
//! only once the window exists, and Windows rescales a window that crosses onto a
//! monitor of another scale. So a window is asked for its place in the bar's points
//! first, then moved and sized again from inside, with its own scale, until it sits
//! where it should. It stays hidden until then.

use cb_core::Rect;
use egui::{pos2, vec2, Context, Pos2, Vec2, ViewportBuilder, ViewportCommand};

/// The point value that egui converts back to exactly `pixel`. Its conversion may
/// truncate, so a plain division can land one pixel short.
pub fn points_for_pixel(pixel: i32, ppp: f32) -> f32 {
    let nudge = if pixel < 0 { -0.25 } else { 0.25 };
    (pixel as f32 + nudge) / ppp
}

fn position(rect: Rect, ppp: f32) -> Pos2 {
    pos2(points_for_pixel(rect.x, ppp), points_for_pixel(rect.y, ppp))
}

fn size(rect: Rect, ppp: f32) -> Vec2 {
    vec2(
        points_for_pixel(rect.width as i32, ppp),
        points_for_pixel(rect.height as i32, ppp),
    )
}

/// `builder` asking for `rect` (global physical pixels), shown only once `visible`.
/// `ctx` is the bar's: its scale is only a first guess for the new window's.
pub(crate) fn placed(
    builder: ViewportBuilder,
    ctx: &Context,
    rect: Rect,
    visible: bool,
) -> ViewportBuilder {
    let ppp = ctx.pixels_per_point();
    builder
        .with_position(position(rect, ppp))
        .with_inner_size(size(rect, ppp))
        .with_visible(visible)
}

/// Inside the viewport: whether its window covers `rect`; if not, move and size it
/// there with its own scale.
pub(crate) fn keep_at(ctx: &Context, rect: Rect) -> bool {
    let ppp = ctx.pixels_per_point();
    let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    let (Some(outer), Some(inner)) = (outer, inner) else {
        return false;
    };
    let near = |points: f32, pixels: i32| ((points * ppp).round() as i32 - pixels).abs() <= 1;
    let there = near(outer.min.x, rect.x)
        && near(outer.min.y, rect.y)
        && near(inner.width(), rect.width as i32)
        && near(inner.height(), rect.height as i32);
    if !there {
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(position(rect, ppp)));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size(rect, ppp)));
    }
    there
}

/// `size` points at the scale of `monitor`, at most 90% of its work area, centred on it,
/// in global physical pixels.
pub fn centred_on(monitor: &cb_core::DisplayInfo, size: Vec2) -> Rect {
    let area = monitor.work_area;
    let width = (size.x * monitor.scale_factor).min(area.width as f32 * 0.9) as u32;
    let height = (size.y * monitor.scale_factor).min(area.height as f32 * 0.9) as u32;
    Rect {
        x: area.x + (area.width.saturating_sub(width) / 2) as i32,
        y: area.y + (area.height.saturating_sub(height) / 2) as i32,
        width,
        height,
    }
}

/// Frames a window may take to get in place before it is shown wherever it is.
const SETTLE_TRIES: u8 = 30;

/// Opening a window the user can then move and resize (Quick Look, Settings, the
/// editor) on a given monitor: it stays hidden until it is there, then is left alone.
#[derive(Debug, Clone, Copy, Default)]
pub struct Opening {
    rect: Option<Rect>,
    tries: u8,
    /// The OS places it: it gets its size in the bar's points.
    unplaced: bool,
}

impl Opening {
    /// At `rect`, in global physical pixels; `None` lets the OS place it.
    pub fn at(rect: Option<Rect>) -> Self {
        Self {
            rect,
            tries: 0,
            unplaced: rect.is_none(),
        }
    }

    /// The builder of each frame: placed and hidden until settled, then as given, its
    /// size left to the user. `size` (points) only serves when the OS places it.
    pub(crate) fn builder(
        &self,
        builder: ViewportBuilder,
        ctx: &Context,
        size: Vec2,
    ) -> ViewportBuilder {
        match self.rect {
            Some(rect) => placed(builder, ctx, rect, false),
            None if self.unplaced => builder.with_inner_size(size).with_visible(true),
            None => builder.with_visible(true),
        }
    }

    /// Inside the viewport, each frame: moves it into place, then shows and focuses it.
    pub(crate) fn settle(&mut self, ctx: &Context) {
        let Some(rect) = self.rect else {
            return;
        };
        self.tries += 1;
        if keep_at(ctx, rect) || self.tries >= SETTLE_TRIES {
            self.rect = None;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_opens_centred_on_its_monitor_at_its_scale() {
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        let monitor = cb_core::DisplayInfo {
            id: cb_core::DisplayId(2),
            name: String::new(),
            bounds: rect(-2560, 0, 2560, 1440),
            work_area: rect(-2560, 0, 2560, 1400),
            scale_factor: 1.5,
        };
        assert_eq!(
            centred_on(&monitor, vec2(560.0, 640.0)),
            rect(-1700, 220, 840, 960)
        );
        let big = centred_on(&monitor, vec2(4000.0, 4000.0));
        assert_eq!((big.width, big.height), (2304, 1260), "kept on the monitor");
    }

    #[test]
    fn points_convert_back_to_the_same_pixel_at_fractional_scales() {
        for ppp in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25] {
            for pixel in [
                -3840, -1085, -853, -1, 0, 1, 100, 1067, 1439, 1920, 2987, 5119,
            ] {
                let back = points_for_pixel(pixel, ppp) * ppp;
                assert_eq!(back as i32, pixel, "truncating at {ppp}");
                assert_eq!(back.round() as i32, pixel, "rounding at {ppp}");
            }
        }
    }
}
