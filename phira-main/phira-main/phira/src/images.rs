use anyhow::Result;
use image::imageops::thumbnail;
use image::DynamicImage;
use prpr::ext::SafeTexture;
use std::future::Future;
use std::path::Path;

pub const THUMBNAIL_WIDTH: u32 = 347;
pub const THUMBNAIL_HEIGHT: u32 = 200;

pub struct Images;
impl Images {
    pub fn into_texture(tex: (DynamicImage, Option<DynamicImage>)) -> (SafeTexture, SafeTexture) {
        match tex {
            (thumb, Some(full)) => (thumb.into(), full.into()),
            (thumb, None) => {
                let tex: SafeTexture = thumb.into();
                (tex.clone(), tex)
            }
        }
    }

    pub fn thumbnail(image: &DynamicImage) -> DynamicImage {
        let width = (image.width() as f32 / image.height() as f32 * THUMBNAIL_HEIGHT as f32).ceil() as u32;
        DynamicImage::ImageRgba8(thumbnail(image, width, THUMBNAIL_HEIGHT))
    }

    pub async fn local_or_else(path: impl AsRef<Path>, task: impl Future<Output = Result<DynamicImage>>) -> Result<DynamicImage> {
        let path = path.as_ref();
        // The cache is an optimisation, so nothing wrong with it is allowed to cost the picture.
        // A file that exists but will not decode - a truncated write from an interrupted run, or
        // one left behind by a different build - used to be fatal here, and `Illustration::settle`
        // has only `warn!` for that, which this target does not log: a cover sitting intact on disk
        // rendered as a black page with nothing in the log to say why. So the read is an attempt
        // and never a decision, and the save below costs the next run a decode instead of the image.
        if path.exists() {
            match tokio::fs::read(path).await {
                Ok(bytes) => match image::load_from_memory(&bytes) {
                    Ok(image) => return Ok(image),
                    Err(err) => crate::log_line(&format!(
                        "images: cache unreadable at {}, regenerating: {err}",
                        path.display()
                    )),
                },
                Err(err) => crate::log_line(&format!(
                    "images: cache unreadable at {}, regenerating: {err}",
                    path.display()
                )),
            }
        }
        let image = task.await?;
        #[cfg(not(target_env = "ohos"))]
        {
            if let Err(err) = image.save_with_format(path, image::ImageFormat::Jpeg) {
                crate::log_line(&format!("images: cache save failed at {}: {err}", path.display()));
            }
        }
        #[cfg(target_env = "ohos")]
        {
            let rgb_image = DynamicImage::ImageRgb8(image.to_rgb8());
            if let Err(err) = rgb_image.save_with_format(path, image::ImageFormat::Jpeg) {
                crate::log_line(&format!("images: cache save failed at {}: {err}", path.display()));
            }
        }
        Ok(image)
    }
}
