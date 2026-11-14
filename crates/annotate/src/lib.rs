//! Annotations on a capture (US-7.1, FR-27): arrows, lines, boxes, ellipses, text and
//! highlights kept as
//! vector layers in image pixels while editing, with unlimited undo, and drawn into the
//! pixels only when saved.

use cb_core::Frame;
use serde::{Deserialize, Serialize};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};
use vello_cpu::kurbo::{self, BezPath, Cap, Join, Shape as _};
use vello_cpu::peniko::Color;
use vello_cpu::{CompositeMode, PixmapMut, RasterizerSettings, RenderContext, Resources};

/// A position in the capture, in image pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn kurbo(self) -> kurbo::Point {
        kurbo::Point::new(self.x.into(), self.y.into())
    }
}

/// A colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const fn with_alpha(self, alpha: u8) -> Self {
        let [r, g, b, _] = self.0;
        Self([r, g, b, alpha])
    }

    fn vello(self) -> Color {
        let [r, g, b, a] = self.0;
        Color::from_rgba8(r, g, b, a)
    }
}

/// One annotation. Widths and sizes are in image pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// Straight, or curved through `via` (see [`curve`]).
    Arrow {
        from: Point,
        to: Point,
        color: Rgba,
        width: f32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        via: Vec<Point>,
    },
    /// A line with round ends: straight, or curved through `via` (see [`curve`]).
    Line {
        from: Point,
        to: Point,
        color: Rgba,
        width: f32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        via: Vec<Point>,
    },
    /// An outlined box, filled with `fill` when there is one. `min` is the top-left corner.
    Rect {
        min: Point,
        max: Point,
        color: Rgba,
        width: f32,
        #[serde(default)]
        fill: Option<Rgba>,
    },
    /// An outlined ellipse inside the box `min`..`max`, filled with `fill` when there is one.
    Ellipse {
        min: Point,
        max: Point,
        color: Rgba,
        width: f32,
        #[serde(default)]
        fill: Option<Rgba>,
    },
    /// A see-through fill over what matters; `color` carries the transparency.
    Highlight { min: Point, max: Point, color: Rgba },
    /// A freehand line through `points`, in the order drawn.
    Pen {
        points: Vec<Point>,
        color: Rgba,
        width: f32,
    },
    /// One line of text; `at` is the top-left corner of the line.
    Text {
        at: Point,
        text: String,
        color: Rgba,
        size: f32,
    },
    /// Squares of `block` pixels, each one flat colour: the average of what it covers,
    /// annotations drawn before it included (US-7.2). What was under it cannot be read
    /// back from the saved image.
    Pixelate { min: Point, max: Point, block: f32 },
    /// Keep only this part of the image. The last crop counts, applied after everything
    /// else is drawn.
    Crop { min: Point, max: Point },
    /// Round the image's corners by `radius` pixels, after any crop: what is cut off
    /// becomes transparent. The last one counts; 0 keeps square corners.
    Corners { radius: f32 },
    /// A numbered step mark for tutorials: a disc of `color` centred on `at`, with
    /// `number` in its middle in [`step_ink`].
    Step {
        at: Point,
        number: u32,
        color: Rgba,
        radius: f32,
    },
}

/// The colour of a step's number on a disc of `color`: white on dark discs, near-black
/// on light ones.
pub fn step_ink(color: Rgba) -> Rgba {
    let [r, g, b, _] = color.0.map(f32::from);
    if 0.299 * r + 0.587 * g + 0.114 * b > 160.0 {
        Rgba([0x14, 0x14, 0x16, 255])
    } else {
        Rgba([255, 255, 255, 255])
    }
}

/// The size of a step's number, pixels per em, for a disc of `radius`.
pub fn step_text_size(radius: f32) -> f32 {
    radius * 1.15
}

/// The number the next step mark carries: one past the highest so far.
pub fn next_step(shapes: &[Shape]) -> u32 {
    shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Step { number, .. } => Some(*number),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// The full name of the first font in a font file (`Segoe UI Bold`), or `None` when it
/// cannot be read as one.
pub fn font_name(font: &[u8]) -> Option<String> {
    let font = skrifa::FontRef::from_index(font, 0).ok()?;
    let name = font
        .localized_strings(skrifa::string::StringId::FULL_NAME)
        .english_or_first()?
        .to_string();
    (!name.trim().is_empty()).then_some(name)
}

/// The box around a line of text at `at`, `size` pixels per em, measured roughly:
/// an average glyph width, not the font's. The editor measures it with the font.
pub fn text_box(at: Point, text: &str, size: f32) -> (Point, Point) {
    let w = text.chars().count() as f32 * size * 0.55;
    (at, Point::new(at.x + w, at.y + size * 1.2))
}

/// The box around `shape`, top-left and bottom-right, without its line width.
pub fn bounds(shape: &Shape) -> (Point, Point) {
    let around = |points: &mut dyn Iterator<Item = Point>| {
        let first = points.next().unwrap_or_default();
        points.fold((first, first), |(a, b), p| {
            (
                Point::new(a.x.min(p.x), a.y.min(p.y)),
                Point::new(b.x.max(p.x), b.y.max(p.y)),
            )
        })
    };
    match shape {
        Shape::Arrow { from, to, via, .. } | Shape::Line { from, to, via, .. } => {
            around(&mut curve(*from, via, *to).into_iter())
        }
        Shape::Pen { points, .. } => around(&mut points.iter().copied()),
        Shape::Rect { min, max, .. }
        | Shape::Ellipse { min, max, .. }
        | Shape::Highlight { min, max, .. }
        | Shape::Pixelate { min, max, .. }
        | Shape::Crop { min, max } => (*min, *max),
        Shape::Corners { .. } => (Point::default(), Point::default()),
        Shape::Text { at, text, size, .. } => text_box(*at, text, *size),
        Shape::Step { at, radius, .. } => (
            Point::new(at.x - radius, at.y - radius),
            Point::new(at.x + radius, at.y + radius),
        ),
    }
}

/// `shape` moved and stretched so that its box `from` becomes `to` (see [`bounds`]).
/// Line widths stay as they are; text grows with the box's height, and a step mark
/// stays round, growing with the smaller side.
pub fn fit(shape: &Shape, from: (Point, Point), to: (Point, Point)) -> Shape {
    let ratio = |a0: f32, a1: f32, b0: f32, b1: f32| {
        if (a1 - a0).abs() > f32::EPSILON {
            (b1 - b0) / (a1 - a0)
        } else {
            1.0
        }
    };
    let sx = ratio(from.0.x, from.1.x, to.0.x, to.1.x);
    let sy = ratio(from.0.y, from.1.y, to.0.y, to.1.y);
    let map = |p: Point| {
        Point::new(
            to.0.x + (p.x - from.0.x) * sx,
            to.0.y + (p.y - from.0.y) * sy,
        )
    };
    let mut out = shape.clone();
    match &mut out {
        Shape::Arrow { from, to, via, .. } | Shape::Line { from, to, via, .. } => {
            *from = map(*from);
            *to = map(*to);
            via.iter_mut().for_each(|p| *p = map(*p));
        }
        Shape::Pen { points, .. } => points.iter_mut().for_each(|p| *p = map(*p)),
        Shape::Rect { min, max, .. }
        | Shape::Ellipse { min, max, .. }
        | Shape::Highlight { min, max, .. }
        | Shape::Pixelate { min, max, .. }
        | Shape::Crop { min, max } => (*min, *max) = corners(map(*min), map(*max)),
        Shape::Corners { .. } => {}
        Shape::Text { at, size, .. } => {
            // The top-left corner follows the box, wherever a flip put it.
            let (corner, _) = corners(map(from.0), map(from.1));
            *at = Point::new(corner.x + (at.x - from.0.x) * sx.abs(), corner.y);
            *size = (*size * sy.abs()).max(4.0);
        }
        Shape::Step { at, radius, .. } => {
            *at = map(*at);
            *radius = (*radius * sx.abs().min(sy.abs())).max(4.0);
        }
    }
    out
}

/// The opposite corners of a drag, as top-left and bottom-right.
pub fn corners(a: Point, b: Point) -> (Point, Point) {
    (
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Point::new(a.x.max(b.x), a.y.max(b.y)),
    )
}

/// The line from `from` to `to` through the points `via`, as a polyline: straight when
/// there are none, else a smooth curve (Catmull-Rom) that passes through each of them.
/// The editor and the saved image both draw this.
pub fn curve(from: Point, via: &[Point], to: Point) -> Vec<Point> {
    let mut knots = Vec::with_capacity(via.len() + 2);
    knots.push(from);
    knots.extend_from_slice(via);
    knots.push(to);
    if knots.len() == 2 {
        return knots;
    }
    let mut out = vec![from];
    for i in 0..knots.len() - 1 {
        let p0 = knots[i.saturating_sub(1)];
        let (p1, p2) = (knots[i], knots[i + 1]);
        let p3 = knots[(i + 2).min(knots.len() - 1)];
        // chisle: steps by length, about one per 4 pixels.
        let steps = (distance(p1, p2) / 4.0).clamp(4.0, 64.0) as usize;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            let at = |a: f32, b: f32, c: f32, d: f32| {
                0.5 * (2.0 * b
                    + (c - a) * t
                    + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
                    + (3.0 * b - a - 3.0 * c + d) * t3)
            };
            out.push(Point::new(
                at(p0.x, p1.x, p2.x, p3.x),
                at(p0.y, p1.y, p2.y, p3.y),
            ));
        }
    }
    out
}

fn distance(a: Point, b: Point) -> f32 {
    (b.x - a.x).hypot(b.y - a.y)
}

/// The point `back` pixels before the end of `line`, measured along it, and how many of
/// its points come before that one.
fn back_along(line: &[Point], back: f32) -> (Point, usize) {
    let mut left = back;
    for i in (1..line.len()).rev() {
        let (a, b) = (line[i - 1], line[i]);
        let d = distance(a, b);
        if d >= left && d > 0.0 {
            let t = left / d;
            return (Point::new(b.x + (a.x - b.x) * t, b.y + (a.y - b.y) * t), i);
        }
        left -= d;
    }
    (line[0], 1)
}

/// How an arrow is drawn, so the editor and the saved image agree.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrowParts {
    /// The line, from the arrow's start to inside the head.
    pub shaft: Vec<Point>,
    /// Tip, then the two back corners.
    pub head: [Point; 3],
}

/// The arrow along `line` (from [`curve`]), its head at the last point.
pub fn arrow_parts(line: &[Point], width: f32) -> ArrowParts {
    let to = *line.last().expect("a line has two points");
    let length: f32 = line.windows(2).map(|w| distance(w[0], w[1])).sum();
    if length < f32::EPSILON {
        return ArrowParts {
            shaft: vec![to],
            head: [to; 3],
        };
    }
    let head_length = (width * 4.0).max(10.0).min(length);
    let half_width = head_length * 0.6;
    // The head points the way the line comes in over the head's own length.
    let (from, _) = back_along(line, head_length);
    let d = distance(from, to).max(f32::EPSILON);
    let (ux, uy) = ((to.x - from.x) / d, (to.y - from.y) / d);
    let base = Point::new(to.x - ux * head_length, to.y - uy * head_length);
    // Half way into the head: the line's round end stays hidden under it.
    let (end, before) = back_along(line, head_length * 0.5);
    let mut shaft = line[..before].to_vec();
    shaft.push(end);
    ArrowParts {
        shaft,
        head: [
            to,
            Point::new(base.x - uy * half_width, base.y + ux * half_width),
            Point::new(base.x + uy * half_width, base.y - ux * half_width),
        ],
    }
}

/// A line width and a text size that read well on an image this size: thicker on a
/// 4K capture than on a small window.
pub fn default_sizes(width: u32, height: u32) -> (f32, f32) {
    let stroke = (width.min(height) as f32 / 250.0).round().max(3.0);
    (stroke, stroke * 6.0)
}

/// One step that can be undone.
#[derive(Debug, Clone, PartialEq)]
enum Edit {
    /// A shape put on top.
    Add(Shape),
    /// The shape that was at this place in the stack, erased.
    Erase(usize, Shape),
    /// The shape at this place changed from the first to the second.
    Replace(usize, Shape, Shape),
}

/// The annotations so far, with unlimited undo and redo (US-7.1).
#[derive(Debug, Clone, Default)]
pub struct History {
    shapes: Vec<Shape>,
    done: Vec<Edit>,
    undone: Vec<Edit>,
}

impl History {
    /// Annotations saved earlier, back as layers: each can be undone, down to the original.
    pub fn with_shapes(shapes: Vec<Shape>) -> Self {
        Self {
            done: shapes.iter().cloned().map(Edit::Add).collect(),
            shapes,
            undone: Vec::new(),
        }
    }

    /// Undo every annotation at once; redo brings them back one by one.
    pub fn undo_all(&mut self) {
        while self.undo() {}
    }

    /// A new annotation; what was undone can no longer be redone.
    pub fn add(&mut self, shape: Shape) {
        self.apply(Edit::Add(shape));
    }

    /// Erase the shape at `index` (bottom first); undo puts it back in its place.
    pub fn erase(&mut self, index: usize) {
        if let Some(shape) = self.shapes.get(index).cloned() {
            self.apply(Edit::Erase(index, shape));
        }
    }

    /// Change the shape at `index`, as one undo step: a line bent, say.
    pub fn replace(&mut self, index: usize, shape: Shape) {
        if let Some(old) = self.shapes.get(index).cloned() {
            if old != shape {
                self.apply(Edit::Replace(index, old, shape));
            }
        }
    }

    fn apply(&mut self, edit: Edit) {
        self.redo_edit(&edit);
        self.done.push(edit);
        self.undone.clear();
    }

    fn redo_edit(&mut self, edit: &Edit) {
        match edit {
            Edit::Add(shape) => self.shapes.push(shape.clone()),
            Edit::Erase(index, _) => {
                self.shapes.remove(*index);
            }
            Edit::Replace(index, _, new) => self.shapes[*index] = new.clone(),
        }
    }

    /// Returns whether there was something to undo.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.done.pop() else {
            return false;
        };
        match &edit {
            Edit::Add(_) => {
                self.shapes.pop();
            }
            Edit::Erase(index, shape) => self.shapes.insert(*index, shape.clone()),
            Edit::Replace(index, old, _) => self.shapes[*index] = old.clone(),
        }
        self.undone.push(edit);
        true
    }

    /// Returns whether there was something to redo.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.undone.pop() else {
            return false;
        };
        self.redo_edit(&edit);
        self.done.push(edit);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Bottom first: later shapes are drawn over earlier ones.
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }
}

/// The topmost shape the eraser at `p` touches, as its index in `shapes`. `reach` is how
/// far from a line still counts, in image pixels. Crops are not drawn, so never touched.
pub fn shape_at(shapes: &[Shape], p: Point, reach: f32) -> Option<usize> {
    shapes.iter().rposition(|shape| touches(shape, p, reach))
}

fn touches(shape: &Shape, p: Point, reach: f32) -> bool {
    let inside = |min: Point, max: Point, pad: f32| {
        p.x >= min.x - pad && p.x <= max.x + pad && p.y >= min.y - pad && p.y <= max.y + pad
    };
    match shape {
        Shape::Arrow {
            from,
            to,
            width,
            via,
            ..
        }
        | Shape::Line {
            from,
            to,
            width,
            via,
            ..
        } => curve(*from, via, *to)
            .windows(2)
            .any(|w| segment_distance(p, w[0], w[1]) <= width / 2.0 + reach),
        Shape::Pen { points, width, .. } => {
            let near = |d: f32| d <= width / 2.0 + reach;
            match points.as_slice() {
                [one] => near((p.x - one.x).hypot(p.y - one.y)),
                _ => points
                    .windows(2)
                    .any(|w| near(segment_distance(p, w[0], w[1]))),
            }
        }
        Shape::Rect {
            min,
            max,
            width,
            fill,
            ..
        } => {
            let pad = width / 2.0 + reach;
            inside(*min, *max, pad) && (fill.is_some() || !inside(*min, *max, -pad))
        }
        Shape::Ellipse {
            min,
            max,
            width,
            fill,
            ..
        } => {
            let (rx, ry) = ((max.x - min.x) / 2.0, (max.y - min.y) / 2.0);
            let (dx, dy) = (p.x - (min.x + rx), p.y - (min.y + ry));
            // chisle: distance to the outline taken along the radius, close enough to aim.
            let r = (dx / rx.max(0.5)).hypot(dy / ry.max(0.5));
            let off = (r - 1.0) * rx.min(ry);
            let pad = width / 2.0 + reach;
            off <= pad && (fill.is_some() || off >= -pad)
        }
        Shape::Highlight { min, max, .. } | Shape::Pixelate { min, max, .. } => {
            inside(*min, *max, reach)
        }
        Shape::Text { at, text, size, .. } => {
            let (min, max) = text_box(*at, text, *size);
            inside(min, max, reach)
        }
        Shape::Crop { .. } | Shape::Corners { .. } => false,
        Shape::Step { at, radius, .. } => distance(p, *at) <= radius + reach,
    }
}

fn segment_distance(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length2 = dx * dx + dy * dy;
    let t = if length2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

#[derive(Debug, thiserror::Error)]
pub enum FlattenError {
    #[error("the image is too large to annotate ({0}×{1})")]
    TooLarge(u32, u32),
    #[error("the image data does not match its size")]
    BadFrame,
    #[error("the font cannot be read")]
    BadFont,
}

/// The font annotations are written in when the system has none to offer.
pub fn fallback_font() -> &'static [u8] {
    epaint_default_fonts::UBUNTU_LIGHT
}

/// `frame` with `shapes` drawn into its pixels, in order. Text uses `font` (TrueType or
/// OpenType bytes). The result has the frame's size and capture time.
pub fn flatten(frame: &Frame, shapes: &[Shape], font: &[u8]) -> Result<Frame, FlattenError> {
    let too_large = || FlattenError::TooLarge(frame.width, frame.height);
    let width = u16::try_from(frame.width).map_err(|_| too_large())?;
    let height = u16::try_from(frame.height).map_err(|_| too_large())?;
    let mut rgba = frame.rgba.clone();
    if rgba.len() != frame.width as usize * frame.height as usize * 4 {
        return Err(FlattenError::BadFrame);
    }
    if shapes.is_empty() || width == 0 || height == 0 {
        return Ok(Frame { rgba, ..*frame });
    }

    // Vector shapes are drawn in runs: a pixelate works on the pixels below it so far.
    let mut run = Vec::new();
    let mut crop = None;
    let mut round = 0.0;
    for shape in shapes {
        match shape {
            Shape::Pixelate { min, max, block } => {
                render(&mut rgba, width, height, &run, font)?;
                run.clear();
                let m = mosaic(&rgba, frame.width, frame.height, *min, *max, *block);
                m.fill(&mut rgba, frame.width);
            }
            Shape::Crop { min, max } => crop = Some((*min, *max)),
            Shape::Corners { radius } => round = *radius,
            shape => run.push(shape),
        }
    }
    render(&mut rgba, width, height, &run, font)?;
    let out = Frame { rgba, ..*frame };
    let mut out = match crop {
        Some((min, max)) => cropped(&out, min, max),
        None => out,
    };
    round_corners(&mut out, round);
    Ok(out)
}

/// Make the image's corners round, `radius` pixels, with a smooth edge: what lies
/// outside becomes transparent.
fn round_corners(frame: &mut Frame, radius: f32) {
    let (w, h) = (frame.width as f32, frame.height as f32);
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return;
    }
    let side = r.ceil() as u32;
    for y in (0..side.min(frame.height)).chain(frame.height.saturating_sub(side)..frame.height) {
        for x in (0..side.min(frame.width)).chain(frame.width.saturating_sub(side)..frame.width) {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = if px < r {
                r
            } else if px > w - r {
                w - r
            } else {
                continue;
            };
            let cy = if py < r {
                r
            } else if py > h - r {
                h - r
            } else {
                continue;
            };
            let cover = (r - (px - cx).hypot(py - cy) + 0.5).clamp(0.0, 1.0);
            let alpha = &mut frame.rgba[((y * frame.width + x) * 4 + 3) as usize];
            *alpha = (f32::from(*alpha) * cover).round() as u8;
        }
    }
}

/// The corner radius the image is rounded by: the last [`Shape::Corners`], or 0.
pub fn corner_radius(shapes: &[Shape]) -> f32 {
    shapes
        .iter()
        .rev()
        .find_map(|s| match s {
            Shape::Corners { radius } => Some(*radius),
            _ => None,
        })
        .unwrap_or(0.0)
}

fn render(
    rgba: &mut [u8],
    width: u16,
    height: u16,
    shapes: &[&Shape],
    font: &[u8],
) -> Result<(), FlattenError> {
    if shapes.is_empty() {
        return Ok(());
    }
    let mut ctx = RenderContext::new(width, height);
    for shape in shapes {
        draw(&mut ctx, shape, font)?;
    }
    premultiply(rgba);
    let target = PixmapMut::new(width, height, rgba).ok_or(FlattenError::BadFrame)?;
    let settings = RasterizerSettings {
        composite_mode: CompositeMode::SrcOver,
        ..Default::default()
    };
    ctx.render_with(target, &mut Resources::new(), settings);
    unpremultiply(rgba);
    Ok(())
}

/// The whole image pixels a rectangle covers, kept inside a `width`×`height` image:
/// `(x0, y0, x1, y1)`, ends excluded.
pub fn pixel_rect(min: Point, max: Point, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let clamp = |v: f32, limit: u32| v.clamp(0.0, limit as f32) as u32;
    (
        clamp(min.x.floor(), width),
        clamp(min.y.floor(), height),
        clamp(max.x.ceil(), width),
        clamp(max.y.ceil(), height),
    )
}

/// The block colours of a [`Shape::Pixelate`], so the editor shows what will be saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Mosaic {
    /// Top-left pixel of the first block.
    pub x0: u32,
    pub y0: u32,
    /// The covered area ends here (excluded); the last blocks may be cut short.
    pub x1: u32,
    pub y1: u32,
    /// Side of a block, in image pixels.
    pub block: u32,
    pub cols: u32,
    pub rows: u32,
    /// Row by row, straight alpha.
    pub colors: Vec<[u8; 4]>,
}

impl Mosaic {
    /// Paint each block's colour over the pixels it covers.
    fn fill(&self, rgba: &mut [u8], width: u32) {
        for y in self.y0..self.y1 {
            let row = (y - self.y0) / self.block;
            for x in self.x0..self.x1 {
                let color = self.colors[(row * self.cols + (x - self.x0) / self.block) as usize];
                let i = ((y * width + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&color);
            }
        }
    }
}

/// The average colour of each `block`-sized square over `min`..`max` of an RGBA image.
pub fn mosaic(rgba: &[u8], width: u32, height: u32, min: Point, max: Point, block: f32) -> Mosaic {
    let (x0, y0, x1, y1) = pixel_rect(min, max, width, height);
    let block = block.round().max(2.0) as u32;
    let cols = (x1 - x0).div_ceil(block);
    let rows = (y1 - y0).div_ceil(block);
    let mut sums = vec![[0u64; 5]; (cols * rows) as usize];
    for y in y0..y1 {
        let row = (y - y0) / block;
        for x in x0..x1 {
            let sum = &mut sums[(row * cols + (x - x0) / block) as usize];
            let i = ((y * width + x) * 4) as usize;
            for (s, &c) in sum.iter_mut().zip(&rgba[i..i + 4]) {
                *s += u64::from(c);
            }
            sum[4] += 1;
        }
    }
    let colors = sums
        .iter()
        .map(|s| {
            let n = s[4].max(1);
            [0, 1, 2, 3].map(|c| ((s[c] + n / 2) / n) as u8)
        })
        .collect();
    Mosaic {
        x0,
        y0,
        x1,
        y1,
        block,
        cols,
        rows,
        colors,
    }
}

/// `frame` cut down to `min`..`max`; unchanged if that holds no pixel.
fn cropped(frame: &Frame, min: Point, max: Point) -> Frame {
    let (x0, y0, x1, y1) = pixel_rect(min, max, frame.width, frame.height);
    if x1 <= x0 || y1 <= y0 {
        return frame.clone();
    }
    let row = |y: u32| {
        let start = ((y * frame.width + x0) * 4) as usize;
        &frame.rgba[start..start + ((x1 - x0) * 4) as usize]
    };
    Frame {
        width: x1 - x0,
        height: y1 - y0,
        rgba: (y0..y1).flat_map(row).copied().collect(),
        captured_at: frame.captured_at,
    }
}

fn draw(ctx: &mut RenderContext, shape: &Shape, font: &[u8]) -> Result<(), FlattenError> {
    match shape {
        Shape::Arrow {
            from,
            to,
            color,
            width,
            via,
        } => {
            let parts = arrow_parts(&curve(*from, via, *to), *width);
            ctx.set_paint(color.vello());
            ctx.set_stroke(
                kurbo::Stroke::new((*width).into())
                    .with_caps(Cap::Round)
                    .with_join(Join::Round),
            );
            ctx.stroke_path(&polyline(&parts.shaft));
            let mut head = BezPath::new();
            head.move_to(parts.head[0].kurbo());
            head.line_to(parts.head[1].kurbo());
            head.line_to(parts.head[2].kurbo());
            head.close_path();
            ctx.fill_path(&head);
        }
        Shape::Line {
            from,
            to,
            color,
            width,
            via,
        } => {
            ctx.set_paint(color.vello());
            ctx.set_stroke(
                kurbo::Stroke::new((*width).into())
                    .with_caps(Cap::Round)
                    .with_join(Join::Round),
            );
            ctx.stroke_path(&polyline(&curve(*from, via, *to)));
        }
        Shape::Rect {
            min,
            max,
            color,
            width,
            fill,
        } => {
            let rect = kurbo::Rect::from_points(min.kurbo(), max.kurbo());
            if let Some(fill) = fill {
                ctx.set_paint(fill.vello());
                ctx.fill_rect(&rect);
            }
            ctx.set_paint(color.vello());
            ctx.set_stroke(kurbo::Stroke::new((*width).into()).with_join(Join::Miter));
            ctx.stroke_rect(&rect);
        }
        Shape::Ellipse {
            min,
            max,
            color,
            width,
            fill,
        } => {
            let bounds = kurbo::Rect::from_points(min.kurbo(), max.kurbo());
            let ellipse = kurbo::Ellipse::from_rect(bounds).to_path(0.1);
            if let Some(fill) = fill {
                ctx.set_paint(fill.vello());
                ctx.fill_path(&ellipse);
            }
            ctx.set_paint(color.vello());
            ctx.set_stroke(kurbo::Stroke::new((*width).into()));
            ctx.stroke_path(&ellipse);
        }
        Shape::Highlight { min, max, color } => {
            ctx.set_paint(color.vello());
            ctx.fill_rect(&kurbo::Rect::from_points(min.kurbo(), max.kurbo()));
        }
        Shape::Pen {
            points,
            color,
            width,
        } => {
            let path = polyline(points);
            ctx.set_paint(color.vello());
            ctx.set_stroke(
                kurbo::Stroke::new((*width).into())
                    .with_caps(Cap::Round)
                    .with_join(Join::Round),
            );
            ctx.stroke_path(&path);
        }
        Shape::Text {
            at,
            text,
            color,
            size,
        } => {
            let path = text_path(font, text, *at, *size)?;
            ctx.set_paint(color.vello());
            ctx.fill_path(&path);
        }
        Shape::Step {
            at,
            number,
            color,
            radius,
        } => {
            ctx.set_paint(color.vello());
            ctx.fill_path(&kurbo::Circle::new(at.kurbo(), (*radius).into()).to_path(0.1));
            let size = step_text_size(*radius);
            let mut digits = text_path(font, &number.to_string(), Point::default(), size)?;
            // Centred by the drawn outlines, so every number sits in the middle.
            let ink = digits.bounding_box();
            digits.apply_affine(kurbo::Affine::translate(at.kurbo() - ink.center()));
            ctx.set_paint(step_ink(*color).vello());
            ctx.fill_path(&digits);
        }
        // Pixel operations, done by `flatten` between runs of vector shapes.
        Shape::Pixelate { .. } | Shape::Crop { .. } | Shape::Corners { .. } => {}
    }
    Ok(())
}

fn polyline(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    let mut points = points.iter().map(|p| p.kurbo());
    if let Some(first) = points.next() {
        path.move_to(first);
        points.for_each(|p| path.line_to(p));
    }
    path
}

/// The outlines of `text` on one line, its top-left corner at `at`, `size` pixels per em.
/// Glyphs are placed by their advance widths, without kerning.
fn text_path(font: &[u8], text: &str, at: Point, size: f32) -> Result<BezPath, FlattenError> {
    let font = skrifa::FontRef::from_index(font, 0).map_err(|_| FlattenError::BadFont)?;
    let size = Size::new(size);
    let location = LocationRef::default();
    let metrics = font.metrics(size, location);
    let advances = font.glyph_metrics(size, location);
    let charmap = font.charmap();
    let outlines = font.outline_glyphs();

    let mut pen = Pen {
        path: BezPath::new(),
        x: at.x.into(),
        baseline: f64::from(at.y + metrics.ascent),
    };
    for ch in text.chars() {
        let glyph = charmap.map(ch).unwrap_or(GlyphId::NOTDEF);
        if let Some(outline) = outlines.get(glyph) {
            // A glyph that cannot be drawn is left blank rather than failing the save.
            let _ = outline.draw(DrawSettings::unhinted(size, location), &mut pen);
        }
        pen.x += f64::from(advances.advance_width(glyph).unwrap_or(0.0));
    }
    Ok(pen.path)
}

/// Collects glyph outlines (y up, from the pen position) into a path in image space (y down).
struct Pen {
    path: BezPath,
    x: f64,
    baseline: f64,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> kurbo::Point {
        kurbo::Point::new(self.x + f64::from(x), self.baseline - f64::from(y))
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.move_to(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.line_to(p);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (c, p) = (self.at(cx0, cy0), self.at(x, y));
        self.path.quad_to(c, p);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        self.path.curve_to(c0, c1, p);
    }

    fn close(&mut self) {
        self.path.close_path();
    }
}

/// The renderer works on premultiplied pixels; captures are usually opaque, where both agree.
fn premultiply(rgba: &mut [u8]) {
    for px in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .filter(|px| px[3] < 255)
    {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
}

fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .filter(|px| px[3] < 255)
    {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = match a {
                0 => 0,
                a => ((u16::from(*c) * 255 + a / 2) / a).min(255) as u8,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    const GREY: [u8; 4] = [128, 128, 128, 255];
    const RED: Rgba = Rgba([255, 0, 0, 255]);

    fn grey(width: u32, height: u32) -> Frame {
        Frame {
            width,
            height,
            rgba: GREY.repeat((width * height) as usize),
            captured_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * frame.width + x) * 4) as usize;
        frame.rgba[i..i + 4].try_into().unwrap()
    }

    fn flat(shapes: &[Shape]) -> Frame {
        flatten(&grey(200, 100), shapes, fallback_font()).unwrap()
    }

    #[test]
    fn undo_and_redo_have_no_limit_and_a_new_shape_ends_redo() {
        let mut history = History::default();
        let dot = |x: f32| Shape::Highlight {
            min: Point::new(x, 0.0),
            max: Point::new(x + 1.0, 1.0),
            color: RED,
        };
        for i in 0..500 {
            history.add(dot(i as f32));
        }
        while history.undo() {}
        assert!(!history.can_undo());
        assert_eq!(history.shapes().len(), 0);
        while history.redo() {}
        assert_eq!(history.shapes().len(), 500);
        assert_eq!(history.shapes()[499], dot(499.0));

        history.undo();
        history.add(dot(-1.0));
        assert!(!history.can_redo(), "a new shape forgets what was undone");
        assert_eq!(history.shapes().last(), Some(&dot(-1.0)));
    }

    #[test]
    fn nothing_to_draw_keeps_every_pixel() {
        let frame = grey(20, 10);
        assert_eq!(
            flatten(&frame, &[], fallback_font()).unwrap().rgba,
            frame.rgba
        );
    }

    #[test]
    fn a_box_colours_its_outline_and_not_its_inside() {
        let out = flat(&[Shape::Rect {
            min: Point::new(20.0, 20.0),
            max: Point::new(120.0, 80.0),
            color: RED,
            width: 6.0,
            fill: None,
        }]);
        assert_eq!(pixel(&out, 70, 20), RED.0, "on the top edge");
        assert_eq!(pixel(&out, 20, 50), RED.0, "on the left edge");
        assert_eq!(pixel(&out, 70, 50), GREY, "inside");
        assert_eq!(pixel(&out, 5, 5), GREY, "outside");
        assert_eq!((out.width, out.height), (200, 100));
    }

    #[test]
    fn a_filled_box_and_ellipse_colour_their_inside() {
        const BLUE: Rgba = Rgba([0, 0, 255, 255]);
        let out = flat(&[
            Shape::Rect {
                min: Point::new(10.0, 10.0),
                max: Point::new(90.0, 90.0),
                color: RED,
                width: 4.0,
                fill: Some(BLUE),
            },
            Shape::Ellipse {
                min: Point::new(110.0, 10.0),
                max: Point::new(190.0, 90.0),
                color: RED,
                width: 4.0,
                fill: Some(BLUE),
            },
        ]);
        assert_eq!(pixel(&out, 50, 50), BLUE.0, "inside the box");
        assert_eq!(pixel(&out, 50, 10), RED.0, "the box outline stays on top");
        assert_eq!(pixel(&out, 150, 50), BLUE.0, "inside the ellipse");
        assert_eq!(pixel(&out, 150, 10), RED.0, "top of the ellipse");
        assert_eq!(pixel(&out, 112, 12), GREY, "the ellipse leaves its corners");
    }

    #[test]
    fn an_empty_ellipse_and_a_line_draw_only_their_strokes() {
        let out = flat(&[
            Shape::Ellipse {
                min: Point::new(10.0, 10.0),
                max: Point::new(90.0, 90.0),
                color: RED,
                width: 4.0,
                fill: None,
            },
            Shape::Line {
                from: Point::new(110.0, 50.0),
                to: Point::new(190.0, 50.0),
                color: RED,
                width: 4.0,
                via: Vec::new(),
            },
        ]);
        assert_eq!(pixel(&out, 10, 50), RED.0, "left of the ellipse");
        assert_eq!(pixel(&out, 50, 50), GREY, "inside the ellipse");
        assert_eq!(pixel(&out, 150, 50), RED.0, "on the line");
        assert_eq!(pixel(&out, 150, 56), GREY, "beside the line");
    }

    #[test]
    fn the_eraser_finds_the_topmost_shape_and_undo_puts_it_back() {
        let line = |y: f32| Shape::Line {
            from: Point::new(0.0, y),
            to: Point::new(100.0, y),
            color: RED,
            width: 4.0,
            via: Vec::new(),
        };
        let hollow = Shape::Rect {
            min: Point::new(20.0, 20.0),
            max: Point::new(80.0, 80.0),
            color: RED,
            width: 4.0,
            fill: None,
        };
        let mut history = History::default();
        for shape in [line(50.0), hollow.clone(), line(10.0)] {
            history.add(shape);
        }
        let at = |h: &History, x, y| shape_at(h.shapes(), Point::new(x, y), 2.0);
        assert_eq!(
            at(&history, 50.0, 51.0),
            Some(0),
            "the line, through the hollow box"
        );
        assert_eq!(at(&history, 20.0, 30.0), Some(1), "the edge of the box");
        assert_eq!(at(&history, 60.0, 30.0), None, "inside the hollow box");
        assert_eq!(at(&history, 50.0, 10.0), Some(2));

        history.erase(1);
        assert_eq!(history.shapes(), [line(50.0), line(10.0)]);
        history.add(line(90.0));
        assert!(history.undo() && history.undo(), "the add, then the erase");
        assert_eq!(history.shapes(), [line(50.0), hollow.clone(), line(10.0)]);
        assert!(history.redo());
        assert_eq!(history.shapes(), [line(50.0), line(10.0)]);
        history.undo_all();
        assert!(history.shapes().is_empty());
    }

    #[test]
    fn a_box_saved_before_fills_existed_still_reads() {
        let rect = Shape::Rect {
            min: Point::new(1.0, 2.0),
            max: Point::new(3.0, 4.0),
            color: RED,
            width: 2.0,
            fill: None,
        };
        let text = ron::to_string(&rect).unwrap();
        let old = text.replace(",fill:None", "");
        assert_ne!(old, text, "the field was there: {text}");
        assert_eq!(ron::from_str::<Shape>(&old).unwrap(), rect);
    }

    #[test]
    fn an_arrow_has_a_line_and_a_head_at_its_end() {
        let from = Point::new(10.0, 50.0);
        let to = Point::new(190.0, 50.0);
        let out = flat(&[Shape::Arrow {
            from,
            to,
            color: RED,
            width: 4.0,
            via: Vec::new(),
        }]);
        assert_eq!(pixel(&out, 60, 50), RED.0, "on the line");
        assert_eq!(pixel(&out, 60, 56), GREY, "beside the line");
        let parts = arrow_parts(&[from, to], 4.0);
        let wide = parts.head[1].y - parts.head[2].y;
        assert!(wide.abs() > 10.0, "the head is wider than the line");
        assert_eq!(pixel(&out, 180, 54), RED.0, "inside the head, off the line");
        assert_eq!(pixel(&out, 195, 50), GREY, "past the tip");
    }

    #[test]
    fn a_bent_line_goes_through_its_points_and_reads_back() {
        let bent = |via: Vec<Point>| Shape::Line {
            from: Point::new(10.0, 90.0),
            to: Point::new(190.0, 90.0),
            color: RED,
            width: 4.0,
            via,
        };
        let line = bent(vec![Point::new(100.0, 20.0)]);
        let out = flat(std::slice::from_ref(&line));
        assert_eq!(pixel(&out, 100, 20), RED.0, "through the bend");
        assert_eq!(pixel(&out, 100, 90), GREY, "not straight across");
        let text = ron::to_string(&line).unwrap();
        assert_eq!(ron::from_str::<Shape>(&text).unwrap(), line);
        let straight = ron::to_string(&bent(Vec::new())).unwrap();
        assert!(
            !straight.contains("via"),
            "older files stay the same: {straight}"
        );

        let mut history = History::with_shapes(vec![line.clone()]);
        let moved = bent(vec![Point::new(100.0, 150.0)]);
        history.replace(0, moved.clone());
        assert_eq!(history.shapes(), [moved]);
        assert!(history.undo());
        assert_eq!(history.shapes(), [line]);
    }

    #[test]
    fn a_curved_arrow_points_along_its_end() {
        let line = curve(
            Point::new(0.0, 0.0),
            &[Point::new(100.0, 100.0)],
            Point::new(200.0, 0.0),
        );
        let parts = arrow_parts(&line, 4.0);
        let tip = parts.head[0];
        let back = Point::new(
            (parts.head[1].x + parts.head[2].x) / 2.0,
            (parts.head[1].y + parts.head[2].y) / 2.0,
        );
        assert!(back.x < tip.x && back.y > tip.y, "comes up from below-left");
        assert!(distance(*parts.shaft.last().unwrap(), tip) < 20.0);
    }

    #[test]
    fn a_freehand_line_follows_its_points() {
        let out = flat(&[Shape::Pen {
            points: vec![
                Point::new(20.0, 20.0),
                Point::new(100.0, 20.0),
                Point::new(100.0, 80.0),
            ],
            color: RED,
            width: 4.0,
        }]);
        assert_eq!(pixel(&out, 60, 20), RED.0, "first stretch");
        assert_eq!(pixel(&out, 100, 50), RED.0, "after the turn");
        assert_eq!(pixel(&out, 60, 50), GREY, "not across the corner");
    }

    #[test]
    fn a_highlight_tints_and_keeps_what_is_under_it() {
        let out = flat(&[Shape::Highlight {
            min: Point::new(0.0, 0.0),
            max: Point::new(100.0, 100.0),
            color: Rgba([255, 255, 0, 102]),
        }]);
        let [r, g, b, a] = pixel(&out, 50, 50);
        assert!(r > 128 && g > 128 && b < 128, "yellowish: {r} {g} {b}");
        assert!(b > 40, "the grey still shows through: {b}");
        assert_eq!(a, 255);
        assert_eq!(pixel(&out, 150, 50), GREY);
    }

    #[test]
    fn text_is_drawn_where_it_was_placed() {
        let out = flat(&[Shape::Text {
            at: Point::new(10.0, 10.0),
            text: "Hello".into(),
            color: RED,
            size: 40.0,
        }]);
        let changed = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&out, x, y) != GREY)
                .count()
        };
        assert!(changed(10, 110, 10, 60) > 100, "the letters are there");
        assert_eq!(changed(150, 200, 0, 100), 0, "nothing past the text");
        assert_eq!(changed(0, 200, 70, 100), 0, "nothing below the line");
    }

    #[test]
    fn round_corners_turn_transparent_after_the_crop() {
        let crop = Shape::Crop {
            min: Point::new(20.0, 20.0),
            max: Point::new(120.0, 80.0),
        };
        let out = flat(&[crop, Shape::Corners { radius: 20.0 }]);
        assert_eq!((out.width, out.height), (100, 60));
        assert_eq!(pixel(&out, 0, 0)[3], 0, "the corner is cut off");
        assert_eq!(pixel(&out, 99, 59)[3], 0);
        assert_eq!(pixel(&out, 50, 0), GREY, "the edges stay");
        assert_eq!(pixel(&out, 20, 20), GREY, "inside the curve");
        let soft = (0..20)
            .filter(|&x| (1..255).contains(&pixel(&out, x, 3)[3]))
            .count();
        assert!(soft > 0, "a smooth edge");
        assert_eq!(
            corner_radius(&[
                Shape::Corners { radius: 8.0 },
                Shape::Corners { radius: 3.0 }
            ]),
            3.0
        );
        assert_eq!(
            flat(&[Shape::Corners { radius: 0.0 }]).rgba,
            grey(200, 100).rgba
        );
    }

    #[test]
    fn a_shape_moves_and_stretches_into_a_new_box() {
        let arrow = Shape::Arrow {
            from: Point::new(10.0, 10.0),
            to: Point::new(30.0, 50.0),
            color: RED,
            width: 4.0,
            via: vec![Point::new(20.0, 20.0)],
        };
        let from = bounds(&arrow);
        assert_eq!(from, (Point::new(10.0, 10.0), Point::new(30.0, 50.0)));
        // Moved by (5, 5) and twice as wide.
        let to = (Point::new(15.0, 15.0), Point::new(55.0, 55.0));
        let Shape::Arrow {
            from: a,
            to: b,
            via,
            width,
            ..
        } = fit(&arrow, from, to)
        else {
            panic!("still an arrow");
        };
        assert_eq!((a, b), (Point::new(15.0, 15.0), Point::new(55.0, 55.0)));
        assert_eq!(via, [Point::new(35.0, 25.0)]);
        assert_eq!(width, 4.0, "line widths stay");

        let step = Shape::Step {
            at: Point::new(50.0, 50.0),
            number: 1,
            color: RED,
            radius: 10.0,
        };
        let to = (Point::new(0.0, 0.0), Point::new(40.0, 60.0));
        let Shape::Step { at, radius, .. } = fit(&step, bounds(&step), to) else {
            panic!("still a step");
        };
        assert_eq!(
            (at, radius),
            (Point::new(20.0, 30.0), 20.0),
            "round, as big as fits"
        );

        // A box dragged past its other corner is flipped, not inside out.
        let rect = Shape::Rect {
            min: Point::new(10.0, 10.0),
            max: Point::new(20.0, 20.0),
            color: RED,
            width: 2.0,
            fill: None,
        };
        let flipped = (Point::new(30.0, 10.0), Point::new(25.0, 20.0));
        let Shape::Rect { min, max, .. } = fit(&rect, bounds(&rect), flipped) else {
            panic!("still a box");
        };
        assert_eq!((min, max), (Point::new(25.0, 10.0), Point::new(30.0, 20.0)));

        let text = Shape::Text {
            at: Point::new(10.0, 10.0),
            text: "Hi".into(),
            color: RED,
            size: 20.0,
        };
        let (min, max) = bounds(&text);
        let taller = (Point::new(min.x, min.y), Point::new(max.x, max.y + 24.0));
        let Shape::Text { at, size, .. } = fit(&text, (min, max), taller) else {
            panic!("still text");
        };
        assert_eq!((at, size), (Point::new(10.0, 10.0), 40.0));
    }

    #[test]
    fn a_step_is_a_disc_with_its_number_in_the_middle() {
        let out = flat(&[Shape::Step {
            at: Point::new(100.0, 50.0),
            number: 7,
            color: RED,
            radius: 30.0,
        }]);
        assert_eq!(pixel(&out, 100, 50 - 26), RED.0, "the disc");
        assert_eq!(pixel(&out, 100, 50 + 26), RED.0, "the disc");
        assert_eq!(pixel(&out, 100, 50 - 34), GREY, "nothing past it");
        let white = (85..115)
            .flat_map(|x| (35..65).map(move |y| (x, y)))
            .filter(|&(x, y)| pixel(&out, x, y) == [255, 255, 255, 255])
            .count();
        assert!(white > 30, "a white number in the middle: {white}");
    }

    #[test]
    fn steps_count_on_from_the_highest_and_ink_follows_the_disc() {
        let step = |number| Shape::Step {
            at: Point::new(0.0, 0.0),
            number,
            color: RED,
            radius: 10.0,
        };
        assert_eq!(next_step(&[]), 1);
        assert_eq!(next_step(&[step(1), step(4), step(2)]), 5);
        assert_eq!(
            step_ink(Rgba([0xFA, 0xCC, 0x15, 255])).0[0],
            0x14,
            "dark on yellow"
        );
        assert_eq!(step_ink(RED).0[0], 255, "white on red");
        assert_eq!(shape_at(&[step(1)], Point::new(8.0, 0.0), 0.0), Some(0));
        assert_eq!(shape_at(&[step(1)], Point::new(14.0, 0.0), 2.0), None);
    }

    #[test]
    fn a_partly_transparent_image_keeps_its_transparency_where_untouched() {
        let mut frame = grey(4, 1);
        frame.rgba[3] = 100;
        frame.rgba[0] = 200;
        let out = flatten(
            &frame,
            &[Shape::Highlight {
                min: Point::new(2.0, 0.0),
                max: Point::new(4.0, 1.0),
                color: RED,
            }],
            fallback_font(),
        )
        .unwrap();
        let [r, _, _, a] = pixel(&out, 0, 0);
        assert_eq!(a, 100);
        assert!(r.abs_diff(200) <= 2, "round trip: {r}");
        assert_eq!(pixel(&out, 3, 0), RED.0);
    }

    #[test]
    fn a_bad_font_fails_only_when_there_is_text() {
        let rect = Shape::Highlight {
            min: Point::default(),
            max: Point::new(1.0, 1.0),
            color: RED,
        };
        assert!(flatten(&grey(4, 4), &[rect], b"not a font").is_ok());
        let text = Shape::Text {
            at: Point::default(),
            text: "x".into(),
            color: RED,
            size: 10.0,
        };
        assert!(matches!(
            flatten(&grey(4, 4), &[text], b"not a font"),
            Err(FlattenError::BadFont)
        ));
    }

    /// Light and dark stripes one pixel wide: what a pixelate must not keep.
    fn stripes(width: u32, height: u32) -> Frame {
        let mut frame = grey(width, height);
        for (i, px) in frame.rgba.chunks_mut(4).enumerate() {
            let v = if i % 2 == 0 { 0 } else { 255 };
            px[..3].fill(v);
        }
        frame
    }

    #[test]
    fn a_pixelate_flattens_each_block_to_its_average() {
        let pixelate = Shape::Pixelate {
            min: Point::new(0.0, 0.0),
            max: Point::new(10.0, 10.0),
            block: 5.0,
        };
        let out = flatten(&stripes(20, 10), &[pixelate], fallback_font()).unwrap();
        for (x, y) in [(0, 0), (1, 0), (4, 4), (5, 9), (9, 9)] {
            let [r, g, b, a] = pixel(&out, x, y);
            assert!(
                r.abs_diff(128) <= 26 && r == g && g == b && a == 255,
                "{x},{y}: {r}"
            );
        }
        assert_eq!(pixel(&out, 10, 0), [0, 0, 0, 255], "outside is untouched");
        assert_eq!(pixel(&out, 11, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn a_pixelate_cannot_be_undone_from_the_saved_pixels() {
        // Two different images with the same block averages save the same pixels:
        // nothing in the result tells which one was under the pixelate (US-7.2 AC).
        let pixelate = Shape::Pixelate {
            min: Point::new(0.0, 0.0),
            max: Point::new(8.0, 8.0),
            block: 4.0,
        };
        let mut swapped = stripes(8, 8);
        for px in swapped.rgba.chunks_mut(4) {
            px[..3].iter_mut().for_each(|c| *c = 255 - *c);
        }
        let a = flatten(
            &stripes(8, 8),
            std::slice::from_ref(&pixelate),
            fallback_font(),
        )
        .unwrap();
        let b = flatten(&swapped, &[pixelate], fallback_font()).unwrap();
        assert_ne!(stripes(8, 8).rgba, swapped.rgba);
        assert_eq!(a.rgba, b.rgba);
    }

    #[test]
    fn a_pixelate_covers_what_was_drawn_before_it_and_not_after() {
        let line = |y: f32| Shape::Rect {
            min: Point::new(20.0, y),
            max: Point::new(180.0, y + 20.0),
            color: RED,
            width: 4.0,
            fill: None,
        };
        let out = flat(&[
            line(10.0),
            Shape::Pixelate {
                min: Point::new(0.0, 0.0),
                max: Point::new(200.0, 100.0),
                block: 50.0,
            },
            line(60.0),
        ]);
        assert_ne!(pixel(&out, 100, 10), RED.0, "under the pixelate");
        assert_eq!(pixel(&out, 100, 60), RED.0, "drawn after it");
    }

    #[test]
    fn the_last_crop_cuts_the_image_after_drawing() {
        let crop = |x: f32| Shape::Crop {
            min: Point::new(x, 10.0),
            max: Point::new(x + 50.5, 40.0),
        };
        let out = flat(&[
            crop(0.0),
            Shape::Highlight {
                min: Point::new(20.0, 10.0),
                max: Point::new(21.0, 11.0),
                color: RED,
            },
            crop(10.0),
        ]);
        assert_eq!((out.width, out.height), (51, 30), "a part pixel is kept");
        assert_eq!(out.rgba.len(), 51 * 30 * 4);
        assert_eq!(pixel(&out, 10, 0), RED.0, "moved with the crop");
        assert_eq!(pixel(&out, 0, 0), GREY);
    }

    #[test]
    fn a_crop_with_no_pixel_keeps_the_whole_image() {
        let out = flat(&[Shape::Crop {
            min: Point::new(300.0, 0.0),
            max: Point::new(400.0, 50.0),
        }]);
        assert_eq!((out.width, out.height), (200, 100));
    }
}
