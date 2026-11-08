//! Bar icons (M2.1), drawn as vectors so they stay crisp at any DPI and need no icon font.

use egui::{pos2, vec2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind};

/// Side of the square an icon occupies, in points.
pub const SIZE: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Grip,
    /// The grip of the vertical bar.
    GripWide,
    Region,
    Window,
    Screen,
    Camera,
    RecordDot,
    Stop,
    Pause,
    Layers,
    ChevronUp,
    ChevronDown,
    Mic,
    Video,
    Sliders,
    Minus,
    Folder,
    Copy,
    /// Save as: an arrow into a tray.
    Save,
    Pen,
    Trash,
    Pin,
    /// Again: a circling arrow.
    Repeat,
    // Annotation tools (US-7.1).
    ArrowTool,
    BoxTool,
    TextTool,
    Highlighter,
    /// Freehand: a wavy line.
    Pencil,
    Undo,
    Redo,
}

/// Draw `icon` centred on `center`. Coordinates below run from -8 to 8.
pub fn paint(p: &Painter, center: Pos2, icon: Icon, color: Color32) {
    let s = Stroke::new(1.5, color);
    let at = |x: f32, y: f32| center + vec2(x, y);
    let frame = |x0: f32, y0: f32, x1: f32, y1: f32, r: u8| {
        p.rect_stroke(
            Rect::from_min_max(at(x0, y0), at(x1, y1)),
            CornerRadius::same(r),
            s,
            StrokeKind::Middle,
        );
    };
    let block = |x0: f32, y0: f32, x1: f32, y1: f32, r: u8| {
        p.rect_filled(
            Rect::from_min_max(at(x0, y0), at(x1, y1)),
            CornerRadius::same(r),
            color,
        );
    };
    let seg = |x0: f32, y0: f32, x1: f32, y1: f32| {
        p.line_segment([at(x0, y0), at(x1, y1)], s);
    };

    match icon {
        Icon::Grip => {
            for x in [-2.5, 2.5] {
                for y in [-5.0, 0.0, 5.0] {
                    p.circle_filled(at(x, y), 1.4, color);
                }
            }
        }
        Icon::GripWide => {
            for x in [-5.0, 0.0, 5.0] {
                for y in [-2.5, 2.5] {
                    p.circle_filled(at(x, y), 1.4, color);
                }
            }
        }
        Icon::Region => {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                p.line(
                    vec![
                        at(sx * 6.5, sy * 3.0),
                        at(sx * 6.5, sy * 6.5),
                        at(sx * 3.0, sy * 6.5),
                    ],
                    s,
                );
            }
        }
        Icon::Window => {
            frame(-7.0, -6.0, 7.0, 6.0, 2);
            seg(-7.0, -2.0, 7.0, -2.0);
        }
        Icon::Screen => {
            frame(-7.0, -6.0, 7.0, 4.0, 2);
            seg(0.0, 4.0, 0.0, 7.0);
            seg(-3.0, 7.0, 3.0, 7.0);
        }
        Icon::Camera => {
            frame(-7.0, -3.5, 7.0, 6.0, 2);
            p.line(
                vec![at(-3.0, -3.5), at(-2.0, -6.0), at(2.0, -6.0), at(3.0, -3.5)],
                s,
            );
            p.circle_stroke(at(0.0, 1.0), 2.5, s);
        }
        Icon::RecordDot => {
            p.circle_filled(center, 5.5, color);
        }
        Icon::Stop => block(-5.0, -5.0, 5.0, 5.0, 2),
        Icon::Pause => {
            block(-4.5, -5.5, -1.5, 5.5, 1);
            block(1.5, -5.5, 4.5, 5.5, 1);
        }
        Icon::Layers => {
            p.line(
                vec![
                    at(0.0, -7.0),
                    at(7.0, -3.5),
                    at(0.0, 0.0),
                    at(-7.0, -3.5),
                    at(0.0, -7.0),
                ],
                s,
            );
            p.line(vec![at(-7.0, 0.5), at(0.0, 4.0), at(7.0, 0.5)], s);
            p.line(vec![at(-7.0, 3.5), at(0.0, 7.0), at(7.0, 3.5)], s);
        }
        Icon::ChevronUp => p_chevron(p, center, -1.0, s),
        Icon::ChevronDown => p_chevron(p, center, 1.0, s),
        Icon::Mic => {
            frame(-2.5, -7.0, 2.5, 2.0, 2);
            let arc = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 10.0;
                    at(5.0 * a.cos(), 0.5 + 5.0 * a.sin())
                })
                .collect();
            p.line(arc, s);
            seg(0.0, 5.5, 0.0, 7.5);
        }
        Icon::Video => {
            frame(-7.0, -4.5, 3.0, 4.5, 2);
            p.line(
                vec![at(3.0, -1.0), at(7.0, -3.5), at(7.0, 3.5), at(3.0, 1.0)],
                s,
            );
        }
        Icon::Sliders => {
            seg(-7.0, -3.5, 0.0, -3.5);
            p.circle_stroke(at(2.0, -3.5), 2.0, s);
            seg(4.0, -3.5, 7.0, -3.5);
            seg(-7.0, 3.5, -4.0, 3.5);
            p.circle_stroke(at(-2.0, 3.5), 2.0, s);
            seg(0.0, 3.5, 7.0, 3.5);
        }
        Icon::Minus => seg(-5.0, 0.0, 5.0, 0.0),
        Icon::Folder => {
            let outline = [
                (-7.0, -5.5),
                (-2.0, -5.5),
                (0.0, -3.5),
                (7.0, -3.5),
                (7.0, 5.5),
            ];
            let mut points: Vec<_> = outline.iter().map(|&(x, y)| at(x, y)).collect();
            points.push(at(-7.0, 5.5));
            p.add(egui::Shape::closed_line(points, s));
        }
        Icon::Copy => {
            frame(-3.0, -3.0, 7.0, 7.0, 2);
            p.line(vec![at(-7.0, 3.0), at(-7.0, -7.0), at(3.0, -7.0)], s);
        }
        Icon::Save => {
            seg(0.0, -7.0, 0.0, 3.0);
            p.line(vec![at(-3.5, -0.5), at(0.0, 3.0), at(3.5, -0.5)], s);
            p.line(
                vec![at(-7.0, 2.0), at(-7.0, 7.0), at(7.0, 7.0), at(7.0, 2.0)],
                s,
            );
        }
        Icon::Pen => {
            p.add(egui::Shape::closed_line(
                vec![
                    at(-6.5, 6.5),
                    at(-6.0, 3.0),
                    at(3.5, -6.5),
                    at(6.5, -3.5),
                    at(-3.0, 6.0),
                ],
                s,
            ));
            seg(1.0, -4.0, 4.0, -1.0);
        }
        Icon::Repeat => {
            let arc = (0..=12)
                .map(|i| {
                    let a = -0.6 + 4.4 * i as f32 / 12.0;
                    at(6.0 * a.cos(), 6.0 * a.sin())
                })
                .collect::<Vec<_>>();
            let tip = *arc.last().expect("arc has points");
            p.line(arc, s);
            p.line(vec![tip + vec2(-1.0, -4.0), tip, tip + vec2(4.0, -1.5)], s);
        }
        Icon::Pin => {
            p.circle_stroke(at(0.0, -3.0), 3.5, s);
            seg(-5.0, 1.5, 5.0, 1.5);
            seg(0.0, 1.5, 0.0, 7.5);
        }
        Icon::Trash => {
            seg(-7.0, -4.5, 7.0, -4.5);
            p.line(
                vec![at(-2.5, -4.5), at(-2.0, -7.0), at(2.0, -7.0), at(2.5, -4.5)],
                s,
            );
            p.line(
                vec![at(-5.5, -4.5), at(-4.5, 7.0), at(4.5, 7.0), at(5.5, -4.5)],
                s,
            );
            seg(-1.5, -1.5, -1.5, 4.0);
            seg(1.5, -1.5, 1.5, 4.0);
        }
        Icon::ArrowTool => {
            seg(-6.0, 6.0, 6.0, -6.0);
            p.line(vec![at(-0.5, -6.0), at(6.0, -6.0), at(6.0, 0.5)], s);
        }
        Icon::BoxTool => frame(-6.5, -5.0, 6.5, 5.0, 1),
        Icon::TextTool => {
            seg(-6.0, -6.5, 6.0, -6.5);
            seg(0.0, -6.5, 0.0, 6.5);
            seg(-2.5, 6.5, 2.5, 6.5);
        }
        Icon::Highlighter => {
            p.add(egui::Shape::closed_line(
                vec![at(-4.0, 2.0), at(2.5, -4.5), at(5.5, -1.5), at(-1.0, 5.0)],
                s,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![at(-4.0, 2.0), at(-1.0, 5.0), at(-5.5, 5.0)],
                color,
                Stroke::NONE,
            ));
            block(-7.0, 6.5, 7.0, 8.0, 0);
        }
        Icon::Pencil => {
            let wave = (0..=16)
                .map(|i| {
                    let x = -7.0 + 14.0 * i as f32 / 16.0;
                    at(x, 4.0 * (x * 0.55).sin() + x * 0.25)
                })
                .collect();
            p.line(wave, s);
        }
        Icon::Undo => p_turn(p, center, -1.0, s),
        Icon::Redo => p_turn(p, center, 1.0, s),
    }
}

/// Undo (`side` -1) or redo (1): a half turn over the top, its head pointing down.
fn p_turn(p: &Painter, c: Pos2, side: f32, s: Stroke) {
    let arc: Vec<Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 12.0;
            pos2(c.x - side * 5.5 * a.cos(), c.y + 2.0 - 5.5 * a.sin())
        })
        .collect();
    let tip = arc[0];
    p.line(arc, s);
    p.line(vec![tip + vec2(-3.0, -3.0), tip, tip + vec2(3.0, -3.0)], s);
}

/// `dir` is -1 to point up, 1 to point down.
fn p_chevron(p: &Painter, c: Pos2, dir: f32, s: Stroke) {
    p.line(
        vec![
            pos2(c.x - 4.0, c.y - dir * 2.0),
            pos2(c.x, c.y + dir * 2.0),
            pos2(c.x + 4.0, c.y - dir * 2.0),
        ],
        s,
    );
}
