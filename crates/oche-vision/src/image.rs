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

    /// A PNG of the frame, for debugging. The pixel data is stored, not
    /// compressed, so files are as large as the raw frame.
    pub fn encode_png(&self) -> Vec<u8> {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = !0u32;
            for &byte in bytes {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend(data);
            out.extend(&body);
            out.extend(crc32(&body).to_be_bytes());
        }
        let mut raw = Vec::with_capacity((self.width * 3 + 1) * self.height);
        for row in self.data.chunks_exact(self.width * 3) {
            raw.push(0);
            raw.extend(row);
        }
        // zlib stream of stored (uncompressed) deflate blocks.
        let mut zlib = vec![0x78, 0x01];
        let mut blocks = raw.chunks(65_535).peekable();
        while let Some(block) = blocks.next() {
            zlib.push(u8::from(blocks.peek().is_none()));
            zlib.extend((block.len() as u16).to_le_bytes());
            zlib.extend((!(block.len() as u16)).to_le_bytes());
            zlib.extend(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        zlib.extend(((b << 16) | a).to_be_bytes());
        let mut header = Vec::new();
        header.extend((self.width as u32).to_be_bytes());
        header.extend((self.height as u32).to_be_bytes());
        header.extend([8, 2, 0, 0, 0]);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        chunk(&mut png, b"IHDR", &header);
        chunk(&mut png, b"IDAT", &zlib);
        chunk(&mut png, b"IEND", &[]);
        png
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
    #[test]
    fn png_has_the_expected_structure() {
        let mut image = RgbImage::new(3, 2);
        image.set(1, 1, [255, 0, 0]);
        let png = image.encode_png();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(
            &png[png.len() - 8..],
            &[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]
        );
        assert_eq!(&png[16..24], &[0, 0, 0, 3, 0, 0, 0, 2]);
    }

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
