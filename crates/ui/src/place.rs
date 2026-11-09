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

#[cfg(test)]
mod tests {
    use super::*;

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
