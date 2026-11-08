//! Annotations on a capture (US-7.1, FR-27): arrows, boxes, text and highlights kept as
//! vector layers in image pixels while editing, with unlimited undo, and drawn into the
//! pixels only when saved.

use cb_core::Frame;
use serde::{Deserialize, Serialize};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};
use vello_cpu::kurbo::{self, BezPath, Cap, Join, Line, Shape as _};
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
    Arrow {
        from: Point,
        to: Point,
        color: Rgba,
        width: f32,
    },
    /// An outlined box. `min` is the top-left corner.
    Rect {
        min: Point,
        max: Point,
        color: Rgba,
        width: f32,
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
}

/// The opposite corners of a drag, as top-left and bottom-right.
pub fn corners(a: Point, b: Point) -> (Point, Point) {
    (
        Point::new(a.x.min(b.x), a.y.min(b.y)),
        Point::new(a.x.max(b.x), a.y.max(b.y)),
    )
}

/// How an arrow is drawn, so the editor and the saved image agree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowParts {
    /// The line runs from the arrow's start to here, inside the head.
    pub shaft_end: Point,
    /// Tip, then the two back corners.
    pub head: [Point; 3],
}

pub fn arrow_parts(from: Point, to: Point, width: f32) -> ArrowParts {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy);
    if length < f32::EPSILON {
        return ArrowParts {
            shaft_end: to,
            head: [to; 3],
        };
    }
    let (ux, uy) = (dx / length, dy / length);
    let head_length = (width * 4.0).max(10.0).min(length);
    let half_width = head_length * 0.6;
    let base = Point::new(to.x - ux * head_length, to.y - uy * head_length);
    ArrowParts {
        // Half way into the head: the line's round end stays hidden under it.
        shaft_end: Point::new(to.x - ux * head_length * 0.5, to.y - uy * head_length * 0.5),
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

/// The annotations so far, with unlimited undo and redo (US-7.1).
#[derive(Debug, Clone, Default)]
pub struct History {
    shapes: Vec<Shape>,
    undone: Vec<Shape>,
}

impl History {
    /// Annotations saved earlier, back as layers: each can be undone, down to the original.
    pub fn with_shapes(shapes: Vec<Shape>) -> Self {
        Self {
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
        self.shapes.push(shape);
        self.undone.clear();
    }

    /// Returns whether there was something to undo.
    pub fn undo(&mut self) -> bool {
        self.shapes.pop().map(|s| self.undone.push(s)).is_some()
    }

    /// Returns whether there was something to redo.
    pub fn redo(&mut self) -> bool {
        self.undone.pop().map(|s| self.shapes.push(s)).is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.shapes.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Bottom first: later shapes are drawn over earlier ones.
    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }
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
    for shape in shapes {
        match shape {
            Shape::Pixelate { min, max, block } => {
                render(&mut rgba, width, height, &run, font)?;
                run.clear();
                let m = mosaic(&rgba, frame.width, frame.height, *min, *max, *block);
                m.fill(&mut rgba, frame.width);
            }
            Shape::Crop { min, max } => crop = Some((*min, *max)),
            shape => run.push(shape),
        }
    }
    render(&mut rgba, width, height, &run, font)?;
    let out = Frame { rgba, ..*frame };
    Ok(match crop {
        Some((min, max)) => cropped(&out, min, max),
        None => out,
    })
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
        } => {
            let parts = arrow_parts(*from, *to, *width);
            ctx.set_paint(color.vello());
            ctx.set_stroke(kurbo::Stroke::new((*width).into()).with_caps(Cap::Round));
            ctx.stroke_path(&Line::new(from.kurbo(), parts.shaft_end.kurbo()).to_path(0.1));
            let mut head = BezPath::new();
            head.move_to(parts.head[0].kurbo());
            head.line_to(parts.head[1].kurbo());
            head.line_to(parts.head[2].kurbo());
            head.close_path();
            ctx.fill_path(&head);
        }
        Shape::Rect {
            min,
            max,
            color,
            width,
        } => {
            ctx.set_paint(color.vello());
            ctx.set_stroke(kurbo::Stroke::new((*width).into()).with_join(Join::Miter));
            ctx.stroke_rect(&kurbo::Rect::from_points(min.kurbo(), max.kurbo()));
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
            let mut path = BezPath::new();
            let mut points = points.iter().map(|p| p.kurbo());
            if let Some(first) = points.next() {
                path.move_to(first);
                points.for_each(|p| path.line_to(p));
            }
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
        // Pixel operations, done by `flatten` between runs of vector shapes.
        Shape::Pixelate { .. } | Shape::Crop { .. } => {}
    }
    Ok(())
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
        }]);
        assert_eq!(pixel(&out, 70, 20), RED.0, "on the top edge");
        assert_eq!(pixel(&out, 20, 50), RED.0, "on the left edge");
        assert_eq!(pixel(&out, 70, 50), GREY, "inside");
        assert_eq!(pixel(&out, 5, 5), GREY, "outside");
        assert_eq!((out.width, out.height), (200, 100));
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
        }]);
        assert_eq!(pixel(&out, 60, 50), RED.0, "on the line");
        assert_eq!(pixel(&out, 60, 56), GREY, "beside the line");
        let parts = arrow_parts(from, to, 4.0);
        let wide = parts.head[1].y - parts.head[2].y;
        assert!(wide.abs() > 10.0, "the head is wider than the line");
        assert_eq!(pixel(&out, 180, 54), RED.0, "inside the head, off the line");
        assert_eq!(pixel(&out, 195, 50), GREY, "past the tip");
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
