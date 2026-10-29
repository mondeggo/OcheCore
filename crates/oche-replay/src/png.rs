//! Minimal uncompressed PNG writer for debug overlays.
//!
//! Uses stored (uncompressed) deflate blocks, so files are large but the writer
//! needs no compression dependency. Not intended for anything but inspection.

use oche_vision::image::RgbImage;

pub fn encode(image: &RgbImage) -> Vec<u8> {
    let mut raw = Vec::with_capacity((image.width * 3 + 1) * image.height);
    for row in image.data.chunks_exact(image.width * 3) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }

    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(u16::MAX as usize).peekable();
    if blocks.peek().is_none() {
        zlib.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(image.width as u32).to_be_bytes());
    header.extend_from_slice(&(image.height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, no interlace

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", &header[..]), (b"IDAT", &zlib), (b"IEND", &[])] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let crc = crc32(&png[start..]);
        png.extend_from_slice(&crc.to_be_bytes());
    }
    png
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for byte in chunk {
            a += u32::from(*byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_reference_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn encodes_a_well_formed_stream() {
        let mut image = RgbImage::new(2, 1);
        image.set(1, 0, [255, 0, 0]);
        let png = encode(&image);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "IEND CRC");
        // IDAT payload: zlib header, one final stored block of 7 bytes, adler.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        assert_eq!(&png[idat..idat + 8], &[0x78, 0x01, 1, 7, 0, 0xF8, 0xFF, 0]);
    }
}
