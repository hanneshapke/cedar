//! Downscaling the images attached to a run, so a high-resolution photo doesn't fill the model's
//! prompt. An image whose longer side is over the server-wide `max_image_side` setting (an admin
//! sets it with `PUT /v1/admin/settings`) is resized to it and re-encoded when the run is
//! created: as JPEG, or as PNG when it has transparency, renamed to match. Its camera orientation
//! is applied first, since re-encoding drops the EXIF tag that carried it. Smaller images, PDFs
//! and images that can't be decoded are kept as they were sent.

use crate::models::Attachment;
use crate::store::Store;
use anyhow::{Context, Result};
use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use serde_json::json;
use std::io::Cursor;

/// The longer side, in pixels, when no admin has set one: what vision models take without
/// scaling the image down themselves, and what PDF pages are rendered at.
pub const DEFAULT_MAX_SIDE: u32 = 1568;
/// The range the setting accepts.
pub const MIN_MAX_SIDE: u32 = 256;
pub const MAX_MAX_SIDE: u32 = 8192;
/// Where the setting is saved in the settings table.
const SETTING: &str = "max_image_side";
const JPEG_QUALITY: u8 = 85;

/// The saved `max_image_side`, or `None` when it is the default.
pub async fn saved_max_side(store: &Store) -> Result<Option<u32>> {
    let value = store.setting(SETTING).await?;
    Ok(value
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok()))
}

/// The longer side attached images are downscaled to.
pub async fn max_side(store: &Store) -> Result<u32> {
    Ok(saved_max_side(store).await?.unwrap_or(DEFAULT_MAX_SIDE))
}

/// Whether `side` is in the range `max_image_side` accepts; the error says what is.
pub fn check_max_side(side: u32) -> Result<(), String> {
    if (MIN_MAX_SIDE..=MAX_MAX_SIDE).contains(&side) {
        Ok(())
    } else {
        Err(format!(
            "max_image_side must be between {MIN_MAX_SIDE} and {MAX_MAX_SIDE}"
        ))
    }
}

/// Save `max_image_side` (see [`check_max_side`]), or go back to the default with `None`.
pub async fn set_max_side(store: &Store, side: Option<u32>) -> Result<()> {
    store
        .set_setting(SETTING, side.map(|n| json!(n)).as_ref())
        .await
}

/// Downscale every image in `attachments` whose longer side is over `max_side`. Decoding runs
/// off the async threads.
pub async fn downscale_all(attachments: Vec<Attachment>, max_side: u32) -> Vec<Attachment> {
    if !attachments.iter().any(Attachment::is_image) {
        return attachments;
    }
    let work = move || {
        attachments
            .into_iter()
            .map(|a| match downscale(&a, max_side) {
                Ok(Some(small)) => small,
                Ok(None) => a,
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), attachment = %a.name, "could not downscale an attached image; keeping it as it is");
                    a
                }
            })
            .collect()
    };
    match tokio::task::spawn_blocking(work).await {
        Ok(done) => done,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// The attachment resized to fit `max_side`, or `None` when it isn't an image or already fits.
fn downscale(a: &Attachment, max_side: u32) -> Result<Option<Attachment>> {
    if !a.is_image() {
        return Ok(None);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&a.data)
        .context("decoding base64")?;
    let reader = || ImageReader::new(Cursor::new(&bytes)).with_guessed_format();
    // The header alone says whether there's anything to do.
    let (width, height) = reader()?
        .into_dimensions()
        .context("reading the image size")?;
    if width.max(height) <= max_side {
        return Ok(None);
    }
    let mut decoder = reader()?.into_decoder().context("decoding the image")?;
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder).context("decoding the image")?;
    img.apply_orientation(orientation);
    let img = img.resize(max_side, max_side, FilterType::CatmullRom);

    let mut out = Vec::new();
    let (media_type, ext) = if img.color().has_alpha() {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)?;
        ("image/png", "png")
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
            .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))?;
        ("image/jpeg", "jpg")
    };
    let name = if media_type == a.media_type {
        a.name.clone()
    } else {
        with_extension(&a.name, ext)
    };
    tracing::debug!(
        attachment = %a.name,
        from = %format!("{width}x{height}"),
        to = %format!("{}x{}", img.width(), img.height()),
        "downscaled an attached image"
    );
    Ok(Some(Attachment {
        name,
        media_type: media_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&out),
    }))
}

/// `name` with its extension replaced by (or, without one, followed by) `ext`.
fn with_extension(name: &str, ext: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    format!("{stem}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgb, RgbImage, Rgba, RgbaImage};

    fn attachment(
        name: &str,
        img: &DynamicImage,
        format: ImageFormat,
        media_type: &str,
    ) -> Attachment {
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), format).unwrap();
        Attachment {
            name: name.into(),
            media_type: media_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    fn decoded(a: &Attachment) -> DynamicImage {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&a.data)
            .unwrap();
        image::load_from_memory(&bytes).unwrap()
    }

    #[tokio::test]
    async fn large_images_are_downscaled_and_small_ones_and_pdfs_are_left_alone() {
        let rgb = |w, h| DynamicImage::ImageRgb8(RgbImage::from_pixel(w, h, Rgb([200, 100, 50])));
        let photo = attachment(
            "photo.jpeg",
            &rgb(3000, 2000),
            ImageFormat::Jpeg,
            "image/jpeg",
        );
        let screenshot = attachment("shot.png", &rgb(1000, 2400), ImageFormat::Png, "image/png");
        let clear = DynamicImage::ImageRgba8(RgbaImage::from_pixel(2000, 500, Rgba([0, 0, 0, 0])));
        let logo = attachment("logo.png", &clear, ImageFormat::Png, "image/png");
        let small = attachment("small.png", &rgb(800, 600), ImageFormat::Png, "image/png");
        let pdf = Attachment {
            name: "doc.pdf".into(),
            media_type: "application/pdf".into(),
            data: "JVBERi0=".into(),
        };

        let out = downscale_all(
            vec![photo, screenshot, logo, small.clone(), pdf.clone()],
            1000,
        )
        .await;

        assert_eq!(decoded(&out[0]).dimensions(), (1000, 667));
        assert_eq!(
            (out[0].name.as_str(), out[0].media_type.as_str()),
            ("photo.jpeg", "image/jpeg")
        );
        // Opaque PNGs become JPEGs, renamed to match; transparent ones stay PNGs.
        assert_eq!(decoded(&out[1]).dimensions(), (417, 1000));
        assert_eq!(
            (out[1].name.as_str(), out[1].media_type.as_str()),
            ("shot.jpg", "image/jpeg")
        );
        assert_eq!(decoded(&out[2]).dimensions(), (1000, 250));
        assert_eq!(
            (out[2].name.as_str(), out[2].media_type.as_str()),
            ("logo.png", "image/png")
        );
        assert_eq!(out[3], small);
        assert_eq!(out[4], pdf);
    }

    #[tokio::test]
    async fn an_image_that_cannot_be_decoded_is_kept() {
        let broken = Attachment {
            name: "x.png".into(),
            media_type: "image/png".into(),
            data: "AAAA".into(),
        };
        assert_eq!(
            downscale_all(vec![broken.clone()], 1000).await,
            vec![broken]
        );
    }

    #[tokio::test]
    async fn the_setting_round_trips_and_is_range_checked() {
        let store = Store::connect_memory().await.unwrap();
        assert_eq!(max_side(&store).await.unwrap(), DEFAULT_MAX_SIDE);
        set_max_side(&store, Some(2048)).await.unwrap();
        assert_eq!(max_side(&store).await.unwrap(), 2048);
        assert!(check_max_side(10).is_err() && check_max_side(9000).is_err());
        assert!(check_max_side(MIN_MAX_SIDE).is_ok() && check_max_side(MAX_MAX_SIDE).is_ok());
        set_max_side(&store, None).await.unwrap();
        assert_eq!(saved_max_side(&store).await.unwrap(), None);
    }

    #[test]
    fn extensions_are_replaced() {
        assert_eq!(with_extension("a.png", "jpg"), "a.jpg");
        assert_eq!(with_extension("a.b.webp", "jpg"), "a.b.jpg");
        assert_eq!(with_extension("noext", "jpg"), "noext.jpg");
        assert_eq!(with_extension(".hidden", "jpg"), ".hidden.jpg");
    }
}
