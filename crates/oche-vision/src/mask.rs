//! Luminance images, change masks, connected regions, and principal axes.

use crate::image::RgbImage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl RgbImage {
    /// BT.601 luma with integer weights.
    pub fn luma(&self) -> GrayImage {
        GrayImage {
            width: self.width,
            height: self.height,
            data: self
                .data
                .chunks_exact(3)
                .map(|p| {
                    ((77 * u32::from(p[0]) + 150 * u32::from(p[1]) + 29 * u32::from(p[2])) >> 8)
                        as u8
                })
                .collect(),
        }
    }
}

impl GrayImage {
    /// Decodes a JPEG straight to luminance, skipping colour conversion.
    pub fn decode_jpeg(bytes: &[u8]) -> Result<Self, String> {
        use zune_jpeg::JpegDecoder;
        use zune_jpeg::zune_core::bytestream::ZCursor;
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        let options = DecoderOptions::default()
            .jpeg_set_out_colorspace(ColorSpace::Luma)
            .set_max_width(crate::image::MAX_SIDE)
            .set_max_height(crate::image::MAX_SIDE);
        let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
        let data = decoder.decode().map_err(|error| format!("{error:?}"))?;
        let info = decoder.info().ok_or("JPEG headers were not decoded")?;
        let (width, height) = (usize::from(info.width), usize::from(info.height));
        if data.len() != width * height {
            return Err(format!(
                "decoded {} bytes for a {width}x{height} luma frame",
                data.len()
            ));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// Bilinear sample; `None` unless all four neighbours are inside.
    pub fn bilinear(&self, x: f64, y: f64) -> Option<f64> {
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        if x0 + 1 >= self.width || y0 + 1 >= self.height {
            return None;
        }
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let at = |x: usize, y: usize| f64::from(self.data[y * self.width + x]);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        Some(top * (1.0 - fy) + bottom * fy)
    }

    /// Block average by an integer factor; trailing partial blocks are dropped.
    pub fn downsample(&self, factor: usize) -> Self {
        let (width, height) = (self.width / factor, self.height / factor);
        let mut data = Vec::with_capacity(width * height);
        for by in 0..height {
            for bx in 0..width {
                let mut sum = 0u32;
                for y in by * factor..(by + 1) * factor {
                    let row = &self.data[y * self.width + bx * factor..][..factor];
                    sum += row.iter().map(|v| u32::from(*v)).sum::<u32>();
                }
                data.push((sum / (factor * factor) as u32) as u8);
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// Per-pixel mean of equally sized images.
    pub fn mean(images: &[&Self]) -> Option<Self> {
        let first = images.first()?;
        if images
            .iter()
            .any(|i| i.width != first.width || i.height != first.height)
        {
            return None;
        }
        let n = images.len() as u32;
        let data = (0..first.data.len())
            .map(|i| {
                ((images.iter().map(|img| u32::from(img.data[i])).sum::<u32>() + n / 2) / n) as u8
            })
            .collect();
        Some(Self {
            width: first.width,
            height: first.height,
            data,
        })
    }

    /// Pixels whose absolute difference exceeds `threshold`.
    pub fn changed(&self, other: &Self, threshold: u8) -> Vec<bool> {
        assert_eq!((self.width, self.height), (other.width, other.height));
        self.data
            .iter()
            .zip(&other.data)
            .map(|(a, b)| a.abs_diff(*b) > threshold)
            .collect()
    }
}

/// Why [`dart_ends`] gave no answer, with the measurements behind it.
#[derive(Debug, Clone, PartialEq)]
pub enum TipError {
    SizeMismatch,
    /// No changed group reached `min_pixels`; `largest` is the biggest found.
    TooFewPixels {
        changed: usize,
        largest: usize,
    },
    /// The largest changed group is not long and thin.
    NotElongated {
        pixels: usize,
        length_over_width: f64,
    },
}

impl std::fmt::Display for TipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SizeMismatch => f.write_str("the two frames differ in size"),
            Self::TooFewPixels { changed, largest } => write!(
                f,
                "only {changed} pixels changed, largest group {largest}; the dart is too small or too faint in this view"
            ),
            Self::NotElongated {
                pixels,
                length_over_width,
            } => write!(
                f,
                "the changed shape ({pixels} px) is not dart-like (length/width {length_over_width:.1}, needs 3)"
            ),
        }
    }
}

/// The two ends of the visible part of the largest object that arrived
/// between two frames, in pixels. A black steel tip is invisible on a black
/// board and the flight may be out of frame, so width is no guide to which end
/// is the tip; `tip` is only set when a flight in view settles it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DartEnds {
    pub ends: [(f64, f64); 2],
    /// Index into `ends` of the tip: the end far from a visible flight.
    pub tip: Option<usize>,
}

/// Structures narrower than twice this are not a flight (a barrel is ~10 px).
const FLIGHT_OPENING: usize = 10;
const MIN_FLIGHT_PIXELS: usize = 300;
/// The barrel must reach this far past the flight's own radius.
const MIN_BARREL_PX: f64 = 20.0;

/// Centroid and radius of the wide, solid part of a silhouette (the flight),
/// found by morphological opening: the barrel and shaft are too thin to survive.
fn flight_of(pixels: &[(usize, usize)]) -> Option<((f64, f64), f64)> {
    let radius = FLIGHT_OPENING;
    let x0 = pixels.iter().map(|p| p.0).min()?;
    let y0 = pixels.iter().map(|p| p.1).min()?;
    let x1 = pixels.iter().map(|p| p.0).max()?;
    let y1 = pixels.iter().map(|p| p.1).max()?;
    let (w, h) = (x1 - x0 + 1 + 2 * radius, y1 - y0 + 1 + 2 * radius);
    let mut solid = vec![false; w * h];
    for &(x, y) in pixels {
        solid[(y - y0 + radius) * w + (x - x0 + radius)] = true;
    }
    let outside: Vec<bool> = solid.iter().map(|v| !v).collect();
    let eroded: Vec<bool> = dilate(&outside, w, h, radius)
        .into_iter()
        .map(|v| !v)
        .collect();
    let opened = dilate(&eroded, w, h, radius);
    let count = opened.iter().filter(|v| **v).count();
    if count < MIN_FLIGHT_PIXELS {
        return None;
    }
    let (mut sx, mut sy) = (0.0, 0.0);
    for (i, _) in opened.iter().enumerate().filter(|(_, v)| **v) {
        sx += (i % w) as f64;
        sy += (i / w) as f64;
    }
    let n = count as f64;
    Some((
        (
            sx / n + x0 as f64 - radius as f64,
            sy / n + y0 as f64 - radius as f64,
        ),
        (n / std::f64::consts::PI).sqrt(),
    ))
}

/// Finds the largest elongated object that arrived between two frames and
/// returns where its silhouette ends.
pub fn dart_ends(
    before: &GrayImage,
    after: &GrayImage,
    threshold: u8,
    min_pixels: usize,
    group_radius: usize,
) -> Result<DartEnds, TipError> {
    if (before.width, before.height) != (after.width, after.height) {
        return Err(TipError::SizeMismatch);
    }
    let mask = before.changed(after, threshold);
    let changed = mask.iter().filter(|m| **m).count();
    let (labels, regions) = label(
        &dilate(&mask, after.width, after.height, group_radius),
        after.width,
    );
    let points_of = |label: u32| {
        labels
            .iter()
            .zip(&mask)
            .enumerate()
            .filter(move |(_, (l, m))| **l == label && **m)
            .map(|(i, _)| ((i % after.width) as f64, (i / after.width) as f64))
    };
    let sizes: Vec<_> = regions
        .iter()
        .map(|r| (r.label, points_of(r.label).count()))
        .collect();
    let largest = sizes.iter().map(|s| s.1).max().unwrap_or(0);
    let (label, _) = sizes
        .into_iter()
        .filter(|(_, count)| *count >= min_pixels)
        .max_by_key(|(_, count)| *count)
        .ok_or(TipError::TooFewPixels { changed, largest })?;
    let points: Vec<_> = points_of(label).collect();
    let axis = principal_axis(points.iter().copied())
        .ok_or(TipError::TooFewPixels { changed, largest })?;
    let ratio = axis.spread.0 / axis.spread.1.max(0.5);
    // A visible flight settles which end is the tip: it is the far one. Grouped
    // pixels close the small gaps in the flight's printed pattern.
    let grouped_pixels: Vec<(usize, usize)> = labels
        .iter()
        .enumerate()
        .filter(|(_, l)| **l == label)
        .map(|(i, _)| (i % after.width, i / after.width))
        .collect();
    if let Some((centre, flight_radius)) = flight_of(&grouped_pixels) {
        let far = points
            .iter()
            .map(|p| (*p, (p.0 - centre.0).hypot(p.1 - centre.1)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((tip, distance)) = far
            && distance - flight_radius >= MIN_BARREL_PX
        {
            return Ok(DartEnds {
                ends: [centre, tip],
                tip: Some(1),
            });
        }
    }
    // A wide flight in view makes the whole silhouette fat; the barrel and
    // shaft alone give the line, so try them first.
    if ratio >= 1.5
        && let Some(shaft) = shaft_axis(&points, &axis, 2.0, 20)
    {
        return Ok(DartEnds {
            ends: shaft.ends,
            tip: None,
        });
    }
    if ratio < 3.0 {
        return Err(TipError::NotElongated {
            pixels: points.len(),
            length_over_width: ratio,
        });
    }
    let (dx, dy) = axis.direction;
    let along = |p: &(f64, f64)| (p.0 - axis.centroid.0) * dx + (p.1 - axis.centroid.1) * dy;
    let (low, high) = points
        .iter()
        .map(along)
        .fold((f64::MAX, f64::MIN), |(lo, hi), t| (lo.min(t), hi.max(t)));
    let end = (high - low) * 0.2;
    // Mean position of the points within the outer fifth at one end.
    let profile = |near_high: bool| {
        let ends: Vec<_> = points
            .iter()
            .filter(|p| {
                let t = along(p);
                if near_high {
                    t >= high - end
                } else {
                    t <= low + end
                }
            })
            .collect();
        let n = ends.len().max(1) as f64;
        let mean = ends.iter().fold((0.0, 0.0), |s, p| (s.0 + p.0, s.1 + p.1));
        (mean.0 / n, mean.1 / n)
    };
    let tip_low = profile(false);
    let tip_high = profile(true);
    Ok(DartEnds {
        ends: [tip_low, tip_high],
        tip: None,
    })
}

/// Sets every pixel within `radius` (Chebyshev) of a set pixel.
pub fn dilate(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut out = vec![false; mask.len()];
    for y in 0..height {
        for x in 0..width {
            if !mask[y * width + x] {
                continue;
            }
            for ny in y.saturating_sub(radius)..(y + radius + 1).min(height) {
                out[ny * width + x.saturating_sub(radius)
                    ..ny * width + (x + radius + 1).min(width)]
                    .fill(true);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// Label in the label map; 0 means unlabelled.
    pub label: u32,
    pub count: usize,
    pub min: (usize, usize),
    pub max: (usize, usize),
    pub sum: (f64, f64),
}

impl Region {
    pub fn bbox_area(&self) -> usize {
        (self.max.0 - self.min.0 + 1) * (self.max.1 - self.min.1 + 1)
    }

    /// Whether this bounding box lies strictly inside `outer`'s.
    pub fn inside(&self, outer: &Region) -> bool {
        self.min.0 > outer.min.0
            && self.min.1 > outer.min.1
            && self.max.0 < outer.max.0
            && self.max.1 < outer.max.1
    }

    pub fn centroid(&self) -> (f64, f64) {
        (
            self.sum.0 / self.count as f64,
            self.sum.1 / self.count as f64,
        )
    }
}

/// 4-connected labelling. Returns the label map and one region per label.
pub fn label(mask: &[bool], width: usize) -> (Vec<u32>, Vec<Region>) {
    let height = mask.len() / width.max(1);
    let mut labels = vec![0u32; mask.len()];
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        let label = regions.len() as u32 + 1;
        let mut region = Region {
            label,
            count: 0,
            min: (usize::MAX, usize::MAX),
            max: (0, 0),
            sum: (0.0, 0.0),
        };
        labels[start] = label;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % width, i / width);
            region.count += 1;
            region.min = (region.min.0.min(x), region.min.1.min(y));
            region.max = (region.max.0.max(x), region.max.1.max(y));
            region.sum = (region.sum.0 + x as f64, region.sum.1 + y as f64);
            let neighbours = [
                (x > 0).then(|| i - 1),
                (x + 1 < width).then(|| i + 1),
                (y > 0).then(|| i - width),
                (y + 1 < height).then(|| i + width),
            ];
            for n in neighbours.into_iter().flatten() {
                if mask[n] && labels[n] == 0 {
                    labels[n] = label;
                    stack.push(n);
                }
            }
        }
        regions.push(region);
    }
    (labels, regions)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    pub centroid: (f64, f64),
    /// Unit direction of the major axis.
    pub direction: (f64, f64),
    /// Standard deviations along and across the major axis, in pixels.
    pub spread: (f64, f64),
}

/// Principal axis of a point set; `None` for fewer than two distinct points.
pub fn principal_axis(points: impl IntoIterator<Item = (f64, f64)>) -> Option<Axis> {
    let points: Vec<_> = points.into_iter().collect();
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let cx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = points.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for (x, y) in &points {
        let (dx, dy) = (x - cx, y - cy);
        xx += dx * dx;
        xy += dx * dy;
        yy += dy * dy;
    }
    let (xx, xy, yy) = (xx / n, xy / n, yy / n);
    let mean = (xx + yy) / 2.0;
    let spread = (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
    let (major, minor) = (mean + spread, (mean - spread).max(0.0));
    if major <= f64::EPSILON {
        return None;
    }
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    Some(Axis {
        centroid: (cx, cy),
        direction: (angle.cos(), angle.sin()),
        spread: (major.sqrt(), minor.sqrt()),
    })
}

/// Centre line of a dart's barrel and shaft, excluding the flight.
#[derive(Debug, Clone, PartialEq)]
pub struct ShaftFit {
    /// Point on the line and unit direction.
    pub point: (f64, f64),
    pub direction: (f64, f64),
    /// Extent of the slices used, along the line.
    pub ends: [(f64, f64); 2],
    /// Midpoints between the two silhouette edges of each used slice.
    pub midpoints: Vec<(f64, f64)>,
    /// RMS distance of the used midpoints from the line, in pixels.
    pub rms_px: f64,
}

/// Fits a dart's centre line from its silhouette pixels.
///
/// The silhouette is cut into one-pixel slices across the `rough` axis. Each
/// slice's two outer edges give a width and a midpoint. Slices wider than
/// `max_width_ratio` times the narrow-quartile width (the flight) are dropped,
/// and a line is fitted through the remaining midpoints with iterative outlier
/// removal. Unlike a principal axis of all pixels, this is not pulled sideways
/// by an asymmetric flight. Returns `None` for fewer than `min_slices` slices.
pub fn shaft_axis(
    points: &[(f64, f64)],
    rough: &Axis,
    max_width_ratio: f64,
    min_slices: usize,
) -> Option<ShaftFit> {
    let (c, d) = (rough.centroid, rough.direction);
    let n = (-d.1, d.0);
    let mut slices: std::collections::BTreeMap<i64, (f64, f64)> = std::collections::BTreeMap::new();
    for &(x, y) in points {
        let (dx, dy) = (x - c.0, y - c.1);
        let t = (dx * d.0 + dy * d.1).floor() as i64;
        let s = dx * n.0 + dy * n.1;
        let entry = slices.entry(t).or_insert((s, s));
        entry.0 = entry.0.min(s);
        entry.1 = entry.1.max(s);
    }
    let mut widths: Vec<f64> = slices.values().map(|(lo, hi)| hi - lo + 1.0).collect();
    if widths.len() < min_slices {
        return None;
    }
    widths.sort_by(f64::total_cmp);
    let narrow = widths[widths.len() / 4];
    let mut samples: Vec<(f64, f64)> = slices
        .iter()
        .filter(|(_, (lo, hi))| hi - lo + 1.0 <= max_width_ratio * narrow)
        .map(|(t, (lo, hi))| (*t as f64 + 0.5, (lo + hi) / 2.0))
        .collect();
    let (mut a, mut b, mut rms) = (0.0, 0.0, 0.0);
    for _ in 0..4 {
        if samples.len() < min_slices {
            return None;
        }
        (a, b) = fit_line(&samples)?;
        let residuals: Vec<f64> = samples.iter().map(|(t, m)| m - (a + b * t)).collect();
        rms = (residuals.iter().map(|r| r * r).sum::<f64>() / residuals.len() as f64).sqrt();
        let limit = (2.5 * rms).max(1.0);
        let before = samples.len();
        samples = samples
            .into_iter()
            .zip(&residuals)
            .filter(|(_, r)| r.abs() <= limit)
            .map(|(s, _)| s)
            .collect();
        if samples.len() == before {
            break;
        }
    }
    let to_image = |t: f64, s: f64| (c.0 + d.0 * t + n.0 * s, c.1 + d.1 * t + n.1 * s);
    let (t0, t1) = samples
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (t, _)| {
            (lo.min(*t), hi.max(*t))
        });
    let mean_t = samples.iter().map(|(t, _)| t).sum::<f64>() / samples.len() as f64;
    let norm = (1.0 + b * b).sqrt();
    Some(ShaftFit {
        point: to_image(mean_t, a + b * mean_t),
        direction: ((d.0 + n.0 * b) / norm, (d.1 + n.1 * b) / norm),
        ends: [to_image(t0, a + b * t0), to_image(t1, a + b * t1)],
        midpoints: samples.iter().map(|(t, m)| to_image(*t, *m)).collect(),
        rms_px: rms,
    })
}

/// Least-squares `m = a + b·t`.
fn fit_line(samples: &[(f64, f64)]) -> Option<(f64, f64)> {
    let n = samples.len() as f64;
    let (st, sm) = samples
        .iter()
        .fold((0.0, 0.0), |(st, sm), (t, m)| (st + t, sm + m));
    let (mt, mm) = (st / n, sm / n);
    let (mut stt, mut stm) = (0.0, 0.0);
    for (t, m) in samples {
        stt += (t - mt) * (t - mt);
        stm += (t - mt) * (m - mm);
    }
    if stt <= f64::EPSILON {
        return None;
    }
    let b = stm / stt;
    Some((mm - b * mt, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic bar along x from x = 50 to 149, 3 px thick. Not a real capture.
    fn bar() -> (GrayImage, GrayImage) {
        let empty = GrayImage {
            width: 200,
            height: 100,
            data: vec![120; 200 * 100],
        };
        let mut with = empty.clone();
        for x in 50..150 {
            for dy in 0..3 {
                with.data[(50 + dy) * 200 + x] = 20;
            }
        }
        (empty, with)
    }

    #[test]
    fn dart_ends_are_the_extremities_of_the_visible_object() {
        let (empty, with) = bar();
        let found = dart_ends(&empty, &with, 30, 80, 4).unwrap();
        let mut xs = found.ends.map(|e| e.0);
        xs.sort_by(f64::total_cmp);
        assert!(
            (xs[0] - 50.0).abs() < 6.0 && (xs[1] - 149.0).abs() < 6.0,
            "{xs:?}"
        );
        assert!(found.ends.iter().all(|e| (e.1 - 51.0).abs() < 1.5));
    }

    #[test]
    fn a_visible_flight_settles_the_tip_as_the_far_end() {
        let (empty, mut with) = bar();
        // 28 px wide flight over the last 25 px of the 100 px bar.
        for x in 125..150 {
            for dy in -12..16 {
                with.data[(50 + dy) as usize * 200 + x] = 20;
            }
        }
        let found = dart_ends(&empty, &with, 30, 80, 4).unwrap();
        let tip = found.ends[found.tip.expect("flight in view")];
        assert!(tip.0 < 62.0 && (tip.1 - 51.0).abs() < 2.0, "{tip:?}");
    }

    #[test]
    fn a_face_on_flight_with_a_short_barrel_still_gives_the_tip() {
        let empty = GrayImage {
            width: 200,
            height: 200,
            data: vec![120; 200 * 200],
        };
        let mut with = empty.clone();
        // 50 x 50 flight at the top, a 4 px barrel running 50 px below it.
        for y in 20..70 {
            for x in 75..125 {
                with.data[y * 200 + x] = 20;
            }
        }
        for y in 70..120 {
            for x in 98..102 {
                with.data[y * 200 + x] = 20;
            }
        }
        let found = dart_ends(&empty, &with, 30, 80, 4).unwrap();
        let tip = found.ends[found.tip.expect("flight in view")];
        assert!(tip.1 > 112.0 && (tip.0 - 99.5).abs() < 4.0, "{tip:?}");
    }

    #[test]
    fn dart_ends_abstain_without_a_dart_or_a_long_thin_shape() {
        let (empty, _) = bar();
        assert!(matches!(
            dart_ends(&empty, &empty, 30, 80, 4),
            Err(TipError::TooFewPixels { .. })
        ));
        let mut blob = empty.clone();
        for y in 40..60 {
            for x in 90..110 {
                blob.data[y * 200 + x] = 20;
            }
        }
        assert!(matches!(
            dart_ends(&empty, &blob, 30, 80, 4),
            Err(TipError::NotElongated { .. })
        ));
    }

    #[test]
    fn luma_downsample_and_mean_preserve_flat_values() {
        let mut rgb = RgbImage::new(4, 4);
        rgb.data.fill(200);
        let gray = rgb.luma();
        assert!(gray.data.iter().all(|v| v.abs_diff(200) <= 1));
        let small = gray.downsample(2);
        assert_eq!((small.width, small.height), (2, 2));
        let dark = GrayImage {
            width: 2,
            height: 2,
            data: vec![0; 4],
        };
        let mean = GrayImage::mean(&[&small, &dark]).unwrap();
        assert!(mean.data.iter().all(|v| v.abs_diff(100) <= 1));
        assert!(GrayImage::mean(&[&gray, &dark]).is_none());
        let ramp = GrayImage {
            width: 2,
            height: 2,
            data: vec![0, 100, 0, 100],
        };
        assert_eq!(ramp.bilinear(0.25, 0.5), Some(25.0));
        assert_eq!(ramp.bilinear(1.0, 0.0), None);
        assert_eq!(ramp.bilinear(-0.1, 0.0), None);
    }

    #[test]
    fn changed_mask_dilation_and_labels_find_separate_regions() {
        let a = GrayImage {
            width: 6,
            height: 3,
            data: vec![10; 18],
        };
        let mut b = a.clone();
        b.data[0] = 50;
        b.data[1] = 50;
        b.data[5] = 15; // below threshold
        b.data[17] = 90;
        let mask = a.changed(&b, 20);
        let (labels, regions) = label(&mask, 6);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].count, 2);
        assert_eq!(labels[17], 2);
        let grown = dilate(&mask, 6, 3, 1);
        assert_eq!(grown.iter().filter(|v| **v).count(), 6 + 4);
    }

    #[test]
    fn principal_axis_follows_a_thick_diagonal_bar() {
        let points: Vec<_> = (0..100)
            .flat_map(|t| {
                (-2..=2).map(move |w| {
                    (
                        f64::from(t) + f64::from(w),
                        f64::from(t) * 0.5 - f64::from(w),
                    )
                })
            })
            .collect();
        let axis = principal_axis(points).unwrap();
        let slope = axis.direction.1 / axis.direction.0;
        assert!((slope - 0.5).abs() < 0.02, "{slope}");
        assert!(axis.spread.0 > 10.0 * axis.spread.1);
        assert!(principal_axis([(1.0, 1.0), (1.0, 1.0)]).is_none());
    }

    /// A synthetic dart along y = 0.3·x + 100 for x in 0..200, 5 px wide,
    /// with a 40 px wide flight hanging to one side over x in 150..200.
    fn dart_with_flight() -> Vec<(f64, f64)> {
        let mut points = Vec::new();
        for x in 0..200 {
            let centre = 0.3 * f64::from(x) + 100.0;
            let (below, above) = if x >= 150 { (-4, 40) } else { (-2, 2) };
            for w in below..=above {
                points.push((f64::from(x), (centre + f64::from(w)).round()));
            }
        }
        points
    }

    #[test]
    fn shaft_axis_ignores_an_asymmetric_flight() {
        let points = dart_with_flight();
        let rough = principal_axis(points.iter().copied()).unwrap();
        // The principal axis is pulled toward the flight near the tip.
        let rough_error = (rough.centroid.1
            + rough.direction.1 / rough.direction.0 * (0.0 - rough.centroid.0))
            - 100.0;
        assert!(rough_error.abs() > 3.0, "{rough_error}");

        let shaft = shaft_axis(&points, &rough, 2.0, 20).unwrap();
        let slope = shaft.direction.1 / shaft.direction.0;
        let at_tip = shaft.point.1 + slope * (0.0 - shaft.point.0);
        assert!((slope - 0.3).abs() < 0.01, "{slope}");
        assert!((at_tip - 100.0).abs() < 1.0, "{at_tip}");
        assert!(
            shaft.midpoints.iter().all(|p| p.0 < 152.0),
            "flight slices dropped"
        );
    }

    #[test]
    fn shaft_axis_needs_enough_slices() {
        let points: Vec<_> = (0..5)
            .flat_map(|x| (0..3).map(move |y| (f64::from(x), f64::from(y))))
            .collect();
        let rough = principal_axis(points.iter().copied()).unwrap();
        assert!(shaft_axis(&points, &rough, 2.0, 20).is_none());
    }
}
