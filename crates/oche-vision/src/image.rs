//! Decoded frames.

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Largest accepted frame side; the cameras' largest mode is 1920x1080.
pub const MAX_SIDE: usize = 4096;

/// 8-bit RGB image, row-major, top-left origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0; width * height * 3],
        }
    }

    pub fn decode_jpeg(bytes: &[u8]) -> Result<Self, String> {
        let options = DecoderOptions::default()
            .jpeg_set_out_colorspace(ColorSpace::RGB)
            .set_max_width(MAX_SIDE)
            .set_max_height(MAX_SIDE);
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
        let data = decoder.decode().map_err(|error| format!("{error:?}"))?;
        let info = decoder.info().ok_or("JPEG headers were not decoded")?;
        let (width, height) = (usize::from(info.width), usize::from(info.height));
        if data.len() != width * height * 3 {
            return Err(format!(
                "decoded {} bytes for a {width}x{height} RGB frame",
                data.len()
            ));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// Nearest pixel at sub-pixel coordinates, or `None` outside the image.
    pub fn sample(&self, x: f64, y: f64) -> Option<[u8; 3]> {
        let (x, y) = (x.round(), y.round());
        (x >= 0.0 && y >= 0.0 && x < self.width as f64 && y < self.height as f64)
            .then(|| self.pixel(x as usize, y as usize))
    }

    pub fn set(&mut self, x: i64, y: i64, colour: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = (y as usize * self.width + x as usize) * 3;
            self.data[i..i + 3].copy_from_slice(&colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_jpeg_input() {
        assert!(RgbImage::decode_jpeg(b"not a jpeg").is_err());
        assert!(RgbImage::decode_jpeg(&[]).is_err());
    }

    #[test]
    fn sampling_and_drawing_stay_inside_the_image() {
        let mut image = RgbImage::new(4, 3);
        image.set(3, 2, [1, 2, 3]);
        image.set(-1, 0, [9, 9, 9]);
        image.set(4, 0, [9, 9, 9]);
        assert_eq!(image.sample(2.6, 1.7), Some([1, 2, 3]));
        assert_eq!(image.sample(-0.6, 0.0), None);
        assert_eq!(image.sample(0.0, 3.0), None);
        assert!(image.data.iter().filter(|v| **v == 9).count() == 0);
    }
}
