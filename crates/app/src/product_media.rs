//! Bounded image decoding and a non-looping product-animation timeline.

use anyhow::{Context, Result, bail, ensure};
use gpui_kit::RenderImage;
use image::{
    AnimationDecoder, DynamicImage, Frame, GenericImageView, ImageDecoder, ImageFormat, Limits,
    RgbaImage,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use std::{io::Cursor, sync::Arc, time::Duration};

const MAX_DIMENSION: u32 = 4096;
// Keep a 2x raster for the 260px product header. Animation composition and
// shared transparent-margin removal happen before resizing.
const MAX_RENDER_DIMENSION: u32 = 520;
const MAX_FRAMES: usize = 256;
const MAX_DECODED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;
const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);

/// CPU-decoded frames. Each render image has exactly one frame so GPUI cannot
/// restart the animation when the final frame remains on screen.
#[derive(Debug)]
pub(super) struct Media {
    frames: Vec<Arc<RenderImage>>,
    frame_ends: Vec<Duration>,
    decoded_bytes: u64,
}

impl Media {
    /// Decode on a worker thread. Static requests use the default PNG image or
    /// the first GIF/WebP frame, irrespective of embedded loop metadata.
    pub(super) fn decode(bytes: &[u8], animated: bool) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_ENCODED_BYTES,
            "product image is too large"
        );
        let format = image::guess_format(bytes).context("unknown product image format")?;
        let limits = decode_limits();
        match format {
            ImageFormat::Png => {
                let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits)?;
                check_dimensions(decoder.dimensions())?;
                if animated && decoder.is_apng()? {
                    Self::animation(bytes, format, MAX_DECODED_BYTES)
                } else {
                    Self::still(DynamicImage::from_decoder(decoder)?)
                }
            }
            ImageFormat::Gif if animated => {
                let mut decoder = GifDecoder::new(Cursor::new(bytes))?;
                decoder.set_limits(limits)?;
                check_dimensions(decoder.dimensions())?;
                Self::animation(bytes, format, MAX_DECODED_BYTES)
            }
            ImageFormat::WebP if animated => {
                let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
                decoder.set_limits(limits)?;
                check_dimensions(decoder.dimensions())?;
                if decoder.has_animation() {
                    Self::animation(bytes, format, MAX_DECODED_BYTES)
                } else {
                    Self::still(DynamicImage::from_decoder(decoder)?)
                }
            }
            ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Jpeg => {
                let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
                reader.limits(limits);
                Self::still(reader.decode()?)
            }
            _ => bail!("unsupported product image format"),
        }
    }

    fn still(image: DynamicImage) -> Result<Self> {
        check_dimensions(image.dimensions())?;
        let mut media = Self::empty();
        let frame = Frame::new(image.into_rgba8());
        let bounds = content_bounds(frame.buffer());
        media.push(crop_frame(frame, bounds), MAX_FRAMES, MAX_DECODED_BYTES)?;
        Ok(media)
    }

    fn animation(bytes: &[u8], format: ImageFormat, max_bytes: u64) -> Result<Self> {
        // First inspect one composited frame at a time. Retaining source-size
        // frames would bypass the bounded, resized-frame storage budget.
        let (bounds, frame_count) = animation_bounds(animation_frames(bytes, format)?, MAX_FRAMES)?;
        // Use one scale for the whole animation. A square bound is conservative
        // for any aspect ratio and keeps long animations within retained memory.
        let render_limit = (max_bytes / (4 * frame_count as u64))
            .isqrt()
            .min(u64::from(MAX_RENDER_DIMENSION)) as u32;
        ensure!(
            render_limit > 0,
            "product animation exceeds the decoded memory limit"
        );
        let mut media = Self::empty();
        for frame in animation_frames(bytes, format)? {
            media.push_sized(
                crop_frame(frame?, bounds),
                MAX_FRAMES,
                max_bytes,
                render_limit,
            )?;
        }
        Ok(media)
    }

    fn empty() -> Self {
        Self {
            frames: Vec::new(),
            frame_ends: Vec::new(),
            decoded_bytes: 0,
        }
    }

    fn push(&mut self, frame: Frame, max_frames: usize, max_bytes: u64) -> Result<()> {
        self.push_sized(frame, max_frames, max_bytes, MAX_RENDER_DIMENSION)
    }

    fn push_sized(
        &mut self,
        mut frame: Frame,
        max_frames: usize,
        max_bytes: u64,
        render_limit: u32,
    ) -> Result<()> {
        ensure!(
            self.frames.len() < max_frames,
            "product animation has too many frames"
        );
        check_dimensions(frame.buffer().dimensions())?;
        if frame.buffer().width() > render_limit || frame.buffer().height() > render_limit {
            let delay = frame.delay();
            let resized = DynamicImage::ImageRgba8(frame.into_buffer())
                .resize(
                    render_limit,
                    render_limit,
                    image::imageops::FilterType::Triangle,
                )
                .into_rgba8();
            frame = Frame::from_parts(resized, 0, 0, delay);
        }
        let bytes = frame.buffer().len() as u64;
        ensure!(
            bytes <= max_bytes.saturating_sub(self.decoded_bytes),
            "product animation exceeds the decoded memory limit"
        );
        let delay = Duration::from(frame.delay()).max(MIN_FRAME_DELAY);
        let end = self.duration().saturating_add(delay);
        // GPUI accepts BGRA pixels. The image crate returns RGBA; retain alpha
        // and the fixed crop of the decoder's already-composited canvas.
        for pixel in frame.buffer_mut().as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        self.frames.push(Arc::new(RenderImage::new(vec![frame])));
        self.frame_ends.push(end);
        self.decoded_bytes += bytes;
        Ok(())
    }

    pub(super) fn first_frame(&self) -> Arc<RenderImage> {
        self.frames[0].clone()
    }

    /// Return the frame for elapsed visible playback time and the delay until
    /// its replacement. The final frame has no deadline and never loops.
    pub(super) fn frame_at(&self, elapsed: Duration) -> (Arc<RenderImage>, Option<Duration>) {
        let index = self
            .frame_ends
            .partition_point(|end| *end <= elapsed)
            .min(self.frame_count() - 1);
        let next = (index + 1 < self.frame_count())
            .then(|| self.frame_ends[index].saturating_sub(elapsed));
        (self.frames[index].clone(), next)
    }

    pub(super) fn duration(&self) -> Duration {
        self.frame_ends.last().copied().unwrap_or_default()
    }

    pub(super) fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub(super) fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }
}

/// Each decoder yields full composited frames; subframe offsets/disposal are
/// resolved by the image library before margins are inspected or removed.
fn animation_frames(bytes: &[u8], format: ImageFormat) -> Result<image::Frames<'_>> {
    let limits = decode_limits();
    Ok(match format {
        ImageFormat::Png => {
            let decoder = PngDecoder::with_limits(Cursor::new(bytes), limits)?;
            check_dimensions(decoder.dimensions())?;
            decoder.apng()?.into_frames()
        }
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(limits)?;
            check_dimensions(decoder.dimensions())?;
            decoder.into_frames()
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
            decoder.set_limits(limits)?;
            check_dimensions(decoder.dimensions())?;
            decoder.into_frames()
        }
        _ => bail!("unsupported product animation format"),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContentBounds {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}
impl ContentBounds {
    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

fn content_bounds(image: &RgbaImage) -> Option<ContentBounds> {
    let mut bounds: Option<ContentBounds> = None;
    for (y, row) in image
        .as_raw()
        .chunks_exact(image.width() as usize * 4)
        .enumerate()
    {
        // Preserve even one-alpha shadows and antialiased edges.
        let pixels = row.as_chunks::<4>().0;
        let Some(left) = pixels.iter().position(|pixel| pixel[3] != 0) else {
            continue;
        };
        let right = pixels.iter().rposition(|pixel| pixel[3] != 0).unwrap() + 1;
        let row_bounds = ContentBounds {
            left: left as u32,
            top: y as u32,
            right: right as u32,
            bottom: y as u32 + 1,
        };
        bounds = Some(bounds.map_or(row_bounds, |bounds| bounds.union(row_bounds)));
    }
    bounds
}

fn animation_bounds(
    frames: image::Frames<'_>,
    max_frames: usize,
) -> Result<(Option<ContentBounds>, usize)> {
    let mut bounds: Option<ContentBounds> = None;
    let mut canvas = None;
    let mut frame_count = 0;
    for (index, frame) in frames.enumerate() {
        ensure!(index < max_frames, "product animation has too many frames");
        let frame = frame?;
        frame_count = index + 1;
        let size = frame.buffer().dimensions();
        check_dimensions(size)?;
        if let Some(canvas) = canvas {
            ensure!(
                canvas == size,
                "product animation changes canvas dimensions"
            );
        } else {
            canvas = Some(size);
        }
        if let Some(frame_bounds) = content_bounds(frame.buffer()) {
            bounds = Some(bounds.map_or(frame_bounds, |bounds| bounds.union(frame_bounds)));
        }
    }
    ensure!(canvas.is_some(), "product animation has no frames");
    Ok((bounds, frame_count))
}

fn crop_frame(frame: Frame, bounds: Option<ContentBounds>) -> Frame {
    let Some(bounds) = bounds else { return frame };
    let buffer = frame.buffer();
    if (bounds.left, bounds.top, bounds.right, bounds.bottom)
        == (0, 0, buffer.width(), buffer.height())
    {
        return frame;
    }
    let cropped = image::imageops::crop_imm(
        buffer,
        bounds.left,
        bounds.top,
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
    )
    .to_image();
    Frame::from_parts(cropped, 0, 0, frame.delay())
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits
}

fn check_dimensions((width, height): (u32, u32)) -> Result<()> {
    ensure!(
        width > 0 && height > 0 && width <= MAX_DIMENSION && height <= MAX_DIMENSION,
        "product image dimensions exceed the supported bounds"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, ImageBuffer, ImageEncoder, Rgba, codecs::png::PngEncoder};

    fn apng() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(2, 0).unwrap();
            encoder.set_frame_delay(1, 10).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[255, 0, 0, 255, 255, 0, 0, 255])
                .unwrap();
            writer.set_frame_delay(1, 5).unwrap();
            writer.set_frame_dimension(1, 1).unwrap();
            writer.set_frame_position(1, 0).unwrap();
            writer.set_blend_op(png::BlendOp::Over).unwrap();
            writer.write_image_data(&[0, 0, 255, 255]).unwrap();
            writer.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn apng_composites_subframes_and_stops_at_the_last_frame() {
        let media = Media::decode(&apng(), true).unwrap();
        assert_eq!(media.frame_count(), 2);
        assert_eq!(media.duration(), Duration::from_millis(300));
        assert_eq!(media.decoded_bytes(), 16);
        let first = media.first_frame();
        assert_eq!(
            first.as_bytes(0),
            Some([0, 0, 255, 255, 0, 0, 255, 255].as_slice())
        );
        assert_eq!(
            media.frame_at(Duration::from_millis(99)).1,
            Some(Duration::from_millis(1))
        );
        let (last, next) = media.frame_at(Duration::from_millis(100));
        assert_eq!(
            last.as_bytes(0),
            Some([0, 0, 255, 255, 255, 0, 0, 255].as_slice())
        );
        assert_eq!(next, None);
        assert_eq!(media.frame_at(Duration::from_secs(60)).0, last);
        assert!(media.frames.iter().all(|frame| frame.frame_count() == 1));
    }

    #[test]
    fn static_requests_never_play_an_apng() {
        let media = Media::decode(&apng(), false).unwrap();
        assert_eq!(media.frame_count(), 1);
        assert_eq!(
            media.first_frame().as_bytes(0),
            Some([0, 0, 255, 255, 0, 0, 255, 255].as_slice())
        );
        assert_eq!(media.frame_at(Duration::ZERO).1, None);
    }

    #[test]
    fn apng_default_thumbnail_is_not_an_animation_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(1, 0).unwrap();
            encoder.set_sep_def_img(true).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 255, 0, 255]).unwrap();
            writer.write_image_data(&[255, 0, 0, 255]).unwrap();
            writer.finish().unwrap();
        }
        let thumbnail = Media::decode(&bytes, false).unwrap();
        let animation = Media::decode(&bytes, true).unwrap();
        assert_eq!(
            thumbnail.first_frame().as_bytes(0),
            Some([0, 255, 0, 255].as_slice())
        );
        assert_eq!(
            animation.first_frame().as_bytes(0),
            Some([0, 0, 255, 255].as_slice())
        );
        assert_eq!(animation.frame_count(), 1);
    }

    #[test]
    fn static_png_keeps_alpha_and_uses_bgra() {
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&[17, 32, 48, 64], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let media = Media::decode(&bytes, true).unwrap();
        assert_eq!(
            media.first_frame().as_bytes(0),
            Some([48, 32, 17, 64].as_slice())
        );
        assert_eq!(media.frame_at(Duration::ZERO).1, None);
        assert!(Media::decode(b"not an image", true).is_err());
        assert!(Media::decode(&bytes[..bytes.len() / 2], true).is_err());
    }

    #[test]
    fn gif_repetition_is_ignored_and_zero_delays_are_bounded() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(Frame::from_parts(
                        ImageBuffer::from_pixel(1, 1, Rgba(color)),
                        0,
                        0,
                        Delay::from_numer_denom_ms(0, 1),
                    ))
                    .unwrap();
            }
        }
        let media = Media::decode(&bytes, true).unwrap();
        assert_eq!(media.duration(), Duration::from_millis(20));
        assert_eq!(media.frame_at(Duration::ZERO).1, Some(MIN_FRAME_DELAY));
        let (last, next) = media.frame_at(MIN_FRAME_DELAY);
        assert_eq!(last.as_bytes(0), Some([255, 0, 0, 255].as_slice()));
        assert_eq!(next, None);
        assert_eq!(Media::decode(&bytes, false).unwrap().frame_count(), 1);
    }

    fn encode_apng(frames: &[RgbaImage]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, frames[0].width(), frames[0].height());
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(frames.len() as u32, 0).unwrap();
            encoder.set_frame_delay(1, 10).unwrap();
            encoder.set_blend_op(png::BlendOp::Source).unwrap();
            let mut writer = encoder.write_header().unwrap();
            for frame in frames {
                writer.write_image_data(frame.as_raw()).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    #[test]
    fn transparent_margins_are_removed_without_discarding_faint_pixels() {
        let mut image = RgbaImage::new(5, 4);
        image.put_pixel(1, 1, Rgba([17, 32, 48, 1]));
        image.put_pixel(3, 2, Rgba([50, 60, 70, 255]));
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(image.as_raw(), 5, 4, image::ExtendedColorType::Rgba8)
            .unwrap();
        let media = Media::decode(&bytes, false).unwrap();
        let first = media.first_frame();
        assert_eq!((first.size(0).width.0, first.size(0).height.0), (3, 2));
        assert_eq!(&first.as_bytes(0).unwrap()[..4], &[48, 32, 17, 1]);
        assert_eq!(&first.as_bytes(0).unwrap()[20..24], &[70, 60, 50, 255]);
        assert_eq!(media.decoded_bytes(), 24);
    }

    #[test]
    fn moving_animation_uses_one_crop_for_every_composited_frame() {
        let mut first = RgbaImage::new(8, 5);
        first.put_pixel(1, 2, Rgba([255, 0, 0, 255]));
        let mut second = RgbaImage::new(8, 5);
        second.put_pixel(6, 2, Rgba([0, 0, 255, 255]));
        let bytes = encode_apng(&[first, second, RgbaImage::new(8, 5)]);
        let media = Media::decode(&bytes, true).unwrap();
        assert_eq!(media.frame_count(), 3);
        assert_eq!(media.duration(), Duration::from_millis(300));
        assert_eq!(media.decoded_bytes(), 3 * 6 * 4);
        for frame in &media.frames {
            assert_eq!((frame.size(0).width.0, frame.size(0).height.0), (6, 1));
        }
        assert_eq!(
            &media.frames[0].as_bytes(0).unwrap()[..4],
            &[0, 0, 255, 255]
        );
        assert_eq!(
            &media.frames[1].as_bytes(0).unwrap()[20..24],
            &[255, 0, 0, 255]
        );
        assert!(
            media.frames[2]
                .as_bytes(0)
                .unwrap()
                .iter()
                .all(|byte| *byte == 0)
        );
        let (last, next) = media.frame_at(Duration::from_secs(60));
        assert_eq!(last, media.frames[2]);
        assert_eq!(next, None);
    }

    #[test]
    fn fully_transparent_media_retains_a_valid_canvas() {
        let bytes = encode_apng(&[RgbaImage::new(7, 3), RgbaImage::new(7, 3)]);
        for animated in [false, true] {
            let media = Media::decode(&bytes, animated).unwrap();
            assert_eq!(media.frame_count(), if animated { 2 } else { 1 });
            for frame in &media.frames {
                assert_eq!((frame.size(0).width.0, frame.size(0).height.0), (7, 3));
                assert!(frame.as_bytes(0).unwrap().iter().all(|byte| *byte == 0));
            }
        }
    }

    #[test]
    fn animation_uniformly_downsizes_to_fit_the_retained_memory_budget() {
        let frames = [
            RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 255])),
            RgbaImage::from_pixel(8, 4, Rgba([0, 0, 255, 255])),
            RgbaImage::from_pixel(8, 4, Rgba([0, 255, 0, 255])),
        ];
        let bytes = encode_apng(&frames);
        let media = Media::animation(&bytes, ImageFormat::Png, 48).unwrap();
        assert_eq!(media.frame_count(), 3);
        assert_eq!(media.duration(), Duration::from_millis(300));
        assert_eq!(media.decoded_bytes(), 24);
        for frame in &media.frames {
            assert_eq!((frame.size(0).width.0, frame.size(0).height.0), (2, 1));
        }
        assert!(Media::animation(&bytes, ImageFormat::Png, 8).is_err());
    }

    #[test]
    fn bounds_scan_rejects_excess_frames_and_inconsistent_canvases() {
        let scan = |dimensions: Vec<(u32, u32)>, max_frames| {
            let frames = dimensions
                .into_iter()
                .map(|(width, height)| Ok(Frame::new(RgbaImage::new(width, height))));
            animation_bounds(image::Frames::new(Box::new(frames)), max_frames)
        };
        assert!(scan(vec![(2, 1), (2, 1)], 1).is_err());
        assert!(scan(vec![(2, 1), (3, 1)], 2).is_err());
        assert!(scan(vec![(MAX_DIMENSION + 1, 1)], 1).is_err());
        assert!(scan(Vec::new(), 2).is_err());
    }

    #[test]
    fn rejects_oversized_dimensions_and_bounded_animation_storage() {
        let mut bytes = Vec::new();
        let width = MAX_DIMENSION + 1;
        PngEncoder::new(&mut bytes)
            .write_image(
                &vec![0; width as usize * 4],
                width,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        assert!(Media::decode(&bytes, false).is_err());
        let frame = || Frame::new(ImageBuffer::from_pixel(2, 1, Rgba([1, 2, 3, 4])));
        let mut media = Media::empty();
        media.push(frame(), 2, 8).unwrap();
        assert!(media.push(frame(), 2, 8).is_err());
        assert!(media.push(frame(), 1, 32).is_err());
        assert_eq!(media.frame_count(), 1);
        assert_eq!(media.decoded_bytes(), 8);
    }

    #[test]
    fn resizes_for_the_display_budget_without_changing_timing_or_aspect_ratio() {
        let mut media = Media::empty();
        let frame = Frame::from_parts(
            ImageBuffer::from_pixel(720, 360, Rgba([17, 32, 48, 64])),
            0,
            0,
            Delay::from_numer_denom_ms(25, 1),
        );
        media.push(frame, MAX_FRAMES, MAX_DECODED_BYTES).unwrap();
        let image = media.first_frame();
        assert_eq!(image.size(0).width.0, 520);
        assert_eq!(image.size(0).height.0, 260);
        assert_eq!(media.decoded_bytes(), 520 * 260 * 4);
        assert_eq!(media.duration(), Duration::from_millis(25));
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[48, 32, 17, 64]);
    }
}
