//! The countdown before a recording starts: a number in a disc, centred on what is
//! about to be recorded, in a small window that lets clicks through.

use crate::place::{keep_at, placed};
use crate::{palette, shortcut, tr};
use cb_core::Shortcut;
use egui::{
    pos2, vec2, Align2, CentralPanel, Color32, Context, FontFamily, FontId, Frame, Shape, Stroke,
    ViewportBuilder, ViewportCommand, ViewportId,
};

/// Diameter of the disc, in points.
const SIZE: f32 = 160.0;
/// Room under the disc for the cancel hint.
const HINT: f32 = 28.0;

/// Draw the countdown for one frame. `center` is in global physical pixels, on a
/// monitor of `scale`; `left` is the time still to wait, out of `total`. The window
/// stays hidden until `dressed` (by the caller, once it exists) and in place; `shown`
/// then turns true.
pub fn show_countdown(
    ctx: &Context,
    center: (i32, i32),
    scale: f32,
    left: f32,
    total: f32,
    dressed: bool,
    shown: &mut bool,
) {
    let size = vec2(SIZE + 80.0, SIZE + HINT) * scale;
    let rect = cb_core::Rect {
        x: center.0 - (size.x / 2.0).round() as i32,
        y: center.1 - (SIZE * scale / 2.0).round() as i32,
        width: size.x.round() as u32,
        height: size.y.round() as u32,
    };
    let builder = ViewportBuilder::default()
        .with_title(tr!("countdown-title"))
        .with_decorations(false)
        .with_transparent(true)
        .with_has_shadow(false)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(false)
        .with_active(false);
    let builder = placed(builder, ctx, rect, *shown);
    ctx.show_viewport_immediate(
        ViewportId::from_hash_of("record countdown"),
        builder,
        |ui, _class| {
            CentralPanel::default()
                .frame(Frame::NONE)
                .show(ui, |ui| draw(ui, left, total));
            // Smooth ring; the main loop alone wakes too rarely.
            ui.ctx().request_repaint();
            // Clicks pass through once it shows: a window made click-through while
            // still hidden is never drawn.
            if *shown {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::MousePassthrough(true));
            }
            *shown |= keep_at(ui.ctx(), rect) && dressed;
        },
    );
}

/// Side of the clap test's square, in points.
const CLAP: f32 = 120.0;

/// Draw the clap test's square (US-10.2) centred on `center`, global physical pixels on a
/// monitor of `scale`: white while `lit`, else black. Placed and shown as the countdown.
pub fn show_clap(
    ctx: &Context,
    center: (i32, i32),
    scale: f32,
    lit: bool,
    dressed: bool,
    shown: &mut bool,
) {
    let side = (CLAP * scale).round() as i32;
    let rect = cb_core::Rect {
        x: center.0 - side / 2,
        y: center.1 - side / 2,
        width: side as u32,
        height: side as u32,
    };
    let builder = ViewportBuilder::default()
        .with_title(tr!("clap-title"))
        .with_decorations(false)
        .with_has_shadow(false)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(false)
        .with_active(false);
    let builder = placed(builder, ctx, rect, *shown);
    ctx.show_viewport_immediate(ViewportId::from_hash_of("clap test"), builder, |ui, _| {
        let fill = if lit { Color32::WHITE } else { Color32::BLACK };
        CentralPanel::default()
            .frame(Frame::NONE.fill(fill))
            .show(ui, |_| {});
        if *shown {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::MousePassthrough(true));
        }
        *shown |= keep_at(ui.ctx(), rect) && dressed;
    });
}

fn draw(ui: &mut egui::Ui, left: f32, total: f32) {
    let area = ui.max_rect();
    let center = pos2(area.center().x, area.top() + SIZE / 2.0);
    let radius = SIZE / 2.0 - 4.0;
    let p = ui.painter();
    p.circle_filled(center, radius, Color32::from_black_alpha(190));

    // A ring that empties with each second.
    let part = left.fract();
    let part = if part == 0.0 && left > 0.0 { 1.0 } else { part };
    let points: Vec<_> = (0..=64)
        .map(|i| {
            let angle =
                -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * part * i as f32 / 64.0;
            center + vec2(angle.cos(), angle.sin()) * (radius - 6.0)
        })
        .collect();
    p.add(Shape::line(points, Stroke::new(5.0, palette::RECORD)));

    let number = (left.ceil() as u32).max(1).min(total.ceil() as u32);
    p.text(
        center,
        Align2::CENTER_CENTER,
        number.to_string(),
        FontId::new(76.0, FontFamily::Proportional),
        Color32::WHITE,
    );
    // On a dark pill: the screen behind may be white.
    let hint = p.layout_no_wrap(
        tr!("countdown-cancel", key = shortcut(Shortcut::Record)),
        FontId::new(13.0, FontFamily::Proportional),
        Color32::WHITE,
    );
    let pill = egui::Rect::from_center_size(
        pos2(center.x, area.bottom() - HINT / 2.0),
        hint.size() + vec2(16.0, 6.0),
    );
    p.rect_filled(
        pill,
        egui::CornerRadius::same(11),
        Color32::from_black_alpha(190),
    );
    p.galley(pill.center() - hint.size() / 2.0, hint, Color32::WHITE);
}
