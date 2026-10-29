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

#[cfg(test)]
mod tests {
    use super::*;

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
}
