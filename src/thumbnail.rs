//! Small artwork thumbnails for the control panel's live card preview.
use anyhow::{Context, Result};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, imageops::FilterType};
use std::io::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mask {
    Square,
    /// Corner radius in pixels.
    Rounded(u32),
    Circle,
}

/// Premultiplied, top-down BGRA pixels, ready for a 32-bit DIB and `AlphaBlend`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Decodes PNG, JPEG or WebP, scales it to cover `width`×`height`, centre-crops,
/// and applies `mask` with a softened edge.
pub fn decode(bytes: &[u8], width: u32, height: u32, mask: Mask) -> Result<Thumbnail> {
    anyhow::ensure!(
        width > 0 && height > 0 && width <= 1024 && height <= 1024,
        "Bad size"
    );
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Unknown image format")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 << 20);
    reader.limits(limits);
    let image = reader.decode().context("Could not decode the image")?;
    let rgba = image
        .resize_to_fill(width, height, FilterType::Triangle)
        .to_rgba8();
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        let coverage = coverage(mask, x, y, width, height);
        let alpha = (a as f32 * coverage).round() as u32;
        let premultiply = |c: u8| ((c as u32 * alpha + 127) / 255) as u8;
        bgra.extend([premultiply(b), premultiply(g), premultiply(r), alpha as u8]);
    }
    Ok(Thumbnail {
        width,
        height,
        bgra,
    })
}

/// How much of pixel (x, y) lies inside the mask, 0.0–1.0, with a 1 px soft edge.
fn coverage(mask: Mask, x: u32, y: u32, width: u32, height: u32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let edge = |distance_outside: f32| (0.5 - distance_outside).clamp(0.0, 1.0);
    match mask {
        Mask::Square => 1.0,
        Mask::Circle => {
            let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
            let radius = cx.min(cy);
            edge(((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - radius)
        }
        Mask::Rounded(radius) => {
            let r = (radius as f32)
                .min(width as f32 / 2.0)
                .min(height as f32 / 2.0);
            let nearest_x = px.clamp(r, width as f32 - r);
            let nearest_y = py.clamp(r, height as f32 - r);
            let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();
            if distance == 0.0 {
                1.0
            } else {
                edge(distance - r)
            }
        }
    }
}

/// Decodes PNG, JPEG or WebP, applies any EXIF orientation, downscales to fit within 512×512,
/// and re-encodes as PNG (stripping any EXIF/GPS metadata while preserving transparency).
pub fn prepare_upload(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("Unsupported image format (use PNG, JPEG, or WebP)")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 << 20);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .context("Could not decode the image")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).context("Could not decode the image")?;
    image.apply_orientation(orientation);
    let resized = if image.width() > 512 || image.height() > 512 {
        image.resize(512, 512, FilterType::Triangle)
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    resized
        .write_to(&mut out, image::ImageFormat::Png)
        .context("Could not encode PNG")?;
    Ok(out.into_inner())
}

/// Downloads (any public HTTPS URL, up to 8 MB) and decodes a thumbnail.
#[cfg(windows)]
pub fn fetch(url: &str, width: u32, height: u32, mask: Mask) -> Result<Thumbnail> {
    anyhow::ensure!(url.starts_with("https://"), "Not an HTTPS image");
    decode(&crate::net::get(url, 8 << 20)?, width, height, mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn prepare_upload_downscales_large_images_to_png() {
        let big = png(1024, 512, [10, 20, 30, 128]);
        let prepared = prepare_upload(&big).unwrap();
        let decoded = ImageReader::new(Cursor::new(&prepared))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 256));
        let small = png(64, 32, [10, 20, 30, 255]);
        let kept = prepare_upload(&small).unwrap();
        let decoded_small = ImageReader::new(Cursor::new(&kept))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded_small.width(), decoded_small.height()), (64, 32));
        assert!(prepare_upload(b"not an image").is_err());
    }

    #[test]
    fn decodes_scales_and_premultiplies() {
        let thumb = decode(&png(40, 20, [255, 0, 0, 255]), 10, 10, Mask::Square).unwrap();
        assert_eq!((thumb.width, thumb.height, thumb.bgra.len()), (10, 10, 400));
        assert_eq!(&thumb.bgra[..4], &[0, 0, 255, 255]);
        let half = decode(&png(4, 4, [200, 100, 50, 128]), 2, 2, Mask::Square).unwrap();
        assert_eq!(&half.bgra[..4], &[25, 50, 100, 128]);
    }

    #[test]
    fn masks_clear_corners_but_keep_centres() {
        let source = png(32, 32, [255, 255, 255, 255]);
        let alpha = |t: &Thumbnail, x: u32, y: u32| t.bgra[((y * t.width + x) * 4 + 3) as usize];
        let circle = decode(&source, 32, 32, Mask::Circle).unwrap();
        assert_eq!((alpha(&circle, 0, 0), alpha(&circle, 16, 16)), (0, 255));
        let rounded = decode(&source, 32, 32, Mask::Rounded(8)).unwrap();
        assert_eq!((alpha(&rounded, 0, 0), alpha(&rounded, 16, 0)), (0, 255));
    }

    #[test]
    fn rejects_junk_and_silly_sizes() {
        assert!(decode(b"not an image", 10, 10, Mask::Square).is_err());
        assert!(decode(&png(2, 2, [0; 4]), 0, 10, Mask::Square).is_err());
    }

    /// Talks to the wiki: `cargo test --lib live_wiki -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_wiki_webp_and_png_decode() {
        let ship = fetch(
            "https://media.starcitizen.tools/thumb/6/6c/Perseus_angled_combat.jpg/480px-Perseus_angled_combat.jpg.webp",
            96,
            96,
            Mask::Rounded(10),
        )
        .unwrap();
        assert_eq!(ship.bgra.len(), 96 * 96 * 4);
        let badge = fetch(
            "https://media.starcitizen.tools/a/ad/Mercenary_Guild_logo_RepUI.png",
            28,
            28,
            Mask::Circle,
        )
        .unwrap();
        assert_eq!(badge.width, 28);
    }
}
