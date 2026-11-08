//! One PDF from several images, one page each, every pixel kept (FR-24, US-6.7).
//! Written by hand: a few objects and a cross-reference table do not need a PDF library.

use crate::{check_size, EncodeError};
use cb_core::Frame;
use std::io::Write;

/// Page size in points for one pixel: images are laid out as on a 96 dpi screen.
const POINTS_PER_PIXEL: f64 = 72.0 / 96.0;

/// Counts what goes through, for the cross-reference table.
struct Counted<W> {
    out: W,
    written: usize,
}

impl<W: Write> Counted<W> {
    fn put(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.out.write_all(bytes)?;
        self.written += bytes.len();
        Ok(())
    }
}

/// RGB on white: PDF images have no alpha without a second mask image, and captures are
/// opaque except, sometimes, the rounded corners of a window.
fn rgb_on_white(frame: &Frame) -> Vec<u8> {
    frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = u32::from(p[3]);
            let over = move |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
            [over(p[0]), over(p[1]), over(p[2])]
        })
        .collect()
}

/// Write `images` as a PDF, one page each, and return the number of pages. Each image is
/// read only when its page is written, so a long list never sits in memory at once.
pub fn write_pdf<W: Write>(
    images: impl IntoIterator<Item = Result<Frame, EncodeError>>,
    out: W,
) -> Result<usize, EncodeError> {
    let mut pdf = Counted { out, written: 0 };
    // offsets[n] is where object n starts. 1 is the catalog, 2 the page tree; both go last.
    let mut offsets = vec![0, 0, 0];
    let mut pages = Vec::new();
    pdf.put(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")?;

    for image in images {
        let image = image?;
        check_size(&image)?;
        let (page, content, picture) = (offsets.len(), offsets.len() + 1, offsets.len() + 2);
        let width = f64::from(image.width) * POINTS_PER_PIXEL;
        let height = f64::from(image.height) * POINTS_PER_PIXEL;

        offsets.push(pdf.written);
        pdf.put(
            format!(
                "{page} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width:.2} {height:.2}] \
                 /Resources << /XObject << /Im0 {picture} 0 R >> >> /Contents {content} 0 R >>\nendobj\n"
            )
            .as_bytes(),
        )?;

        let draw = format!("q {width:.2} 0 0 {height:.2} 0 0 cm /Im0 Do Q");
        offsets.push(pdf.written);
        pdf.put(
            format!(
                "{content} 0 obj\n<< /Length {} >>\nstream\n{draw}\nendstream\nendobj\n",
                draw.len()
            )
            .as_bytes(),
        )?;

        let pixels = miniz_oxide::deflate::compress_to_vec_zlib(&rgb_on_white(&image), 6);
        offsets.push(pdf.written);
        pdf.put(
            format!(
                "{picture} 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} \
                 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                image.width,
                image.height,
                pixels.len()
            )
            .as_bytes(),
        )?;
        pdf.put(&pixels)?;
        pdf.put(b"\nendstream\nendobj\n")?;
        pages.push(page);
    }
    if pages.is_empty() {
        return Err(EncodeError::Nothing);
    }

    offsets[1] = pdf.written;
    pdf.put(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n")?;
    let kids: Vec<String> = pages.iter().map(|p| format!("{p} 0 R")).collect();
    offsets[2] = pdf.written;
    pdf.put(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    )?;

    let xref = pdf.written;
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len());
    for offset in &offsets[1..] {
        table.push_str(&format!("{offset:010} 00000 n \n"));
    }
    table.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len()
    ));
    pdf.put(table.as_bytes())?;
    pdf.out.flush()?;
    Ok(pages.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn image(width: u32, height: u32, seed: u8) -> Frame {
        let rgba = (0..width * height)
            .flat_map(|i| [seed, (i % 251) as u8, (i % 7) as u8 * 30, 255])
            .collect();
        Frame {
            width,
            height,
            rgba,
            captured_at: SystemTime::now(),
        }
    }

    /// The text between `from` and the next `to`, after `at`.
    fn between<'a>(text: &'a [u8], at: usize, from: &[u8], to: &[u8]) -> (usize, &'a [u8]) {
        let find = |hay: &[u8], needle: &[u8], start: usize| {
            start
                + hay[start..]
                    .windows(needle.len())
                    .position(|w| w == needle)
                    .unwrap()
        };
        let start = find(text, from, at) + from.len();
        let end = find(text, to, start);
        (end, &text[start..end])
    }

    #[test]
    fn every_page_keeps_its_pixels_and_the_table_points_at_every_object() {
        let images = [image(3, 2, 10), image(5, 4, 200)];
        let mut out = Vec::new();
        let pages = write_pdf(images.iter().cloned().map(Ok), &mut out).unwrap();
        assert_eq!(pages, 2);
        assert!(out.starts_with(b"%PDF-1.4"));
        assert!(out.ends_with(b"%%EOF\n"));

        // The cross-reference table: each entry is where "<n> 0 obj" starts.
        // Offsets count bytes, and the pixel streams are not text: search the bytes.
        let marker = b"startxref\n";
        let after = out
            .windows(marker.len())
            .rposition(|w| w == marker)
            .unwrap()
            + marker.len();
        let tail = std::str::from_utf8(&out[after..]).unwrap();
        let start: usize = tail.lines().next().unwrap().parse().unwrap();
        let table = std::str::from_utf8(&out[start..]).unwrap();
        assert!(table.starts_with("xref\n0 9\n"));
        for (n, line) in table.lines().skip(3).take(8).enumerate() {
            let offset: usize = line[..10].parse().unwrap();
            let object = format!("{} 0 obj", n + 1);
            assert!(
                out[offset..].starts_with(object.as_bytes()),
                "object {}",
                n + 1
            );
        }
        assert!(out.windows(8).any(|w| w == b"/Count 2"));

        // Each image: native size, and the same pixels once inflated.
        let mut at = 0;
        for expected in &images {
            let (end, header) = between(&out, at, b"/Subtype /Image", b"stream\n");
            let header = String::from_utf8_lossy(header);
            assert!(header.contains(&format!(
                "/Width {} /Height {}",
                expected.width, expected.height
            )));
            let (end, data) = between(&out, end, b"stream\n", b"\nendstream");
            let rgb = miniz_oxide::inflate::decompress_to_vec_zlib(data).unwrap();
            assert_eq!(rgb, rgb_on_white(expected));
            at = end;
        }
    }

    #[test]
    fn transparent_pixels_become_white_and_no_image_is_an_error() {
        let clear = Frame {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
            captured_at: SystemTime::now(),
        };
        assert_eq!(rgb_on_white(&clear), vec![255, 255, 255]);
        assert!(matches!(
            write_pdf([], Vec::new()),
            Err(EncodeError::Nothing)
        ));
        let broken = Frame {
            rgba: vec![1],
            ..clear
        };
        assert!(write_pdf([Ok(broken)], Vec::new()).is_err());
    }
}
