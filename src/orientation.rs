use crate::types::LinearImage;
use rawler::Orientation;

/// Apply a camera orientation to a flat row-major buffer of any `Copy` element,
/// returning the new `(width, height, data)`. Shared by the image itself and by
/// any companion per-pixel buffer (e.g. the highlight uncertainty map) so the two
/// cannot drift out of alignment.
pub fn orient_data<T: Copy>(
    width: usize,
    height: usize,
    mut data: Vec<T>,
    orientation: Orientation,
) -> (usize, usize, Vec<T>) {
    let (transpose, horizontal, vertical) = orientation.to_flips();

    if horizontal {
        for row in data.chunks_exact_mut(width) {
            row.reverse();
        }
    }

    if vertical {
        let half = height / 2;
        for y in 0..half {
            let opposite = height - 1 - y;
            let row_a = y * width;
            let row_b = opposite * width;
            for x in 0..width {
                data.swap(row_a + x, row_b + x);
            }
        }
    }

    if transpose {
        let mut transposed = data.clone();
        for y in 0..height {
            for x in 0..width {
                transposed[x * height + y] = data[y * width + x];
            }
        }
        (height, width, transposed)
    } else {
        (width, height, data)
    }
}

pub fn apply_orientation(image: LinearImage, orientation: Orientation) -> LinearImage {
    let (width, height, pixels) = orient_data(image.width, image.height, image.pixels, orientation);
    LinearImage::new(width, height, pixels).expect("orientation preserves the pixel count")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> LinearImage {
        LinearImage::new(2, 3, (0..6).map(|value| [value as f32; 3]).collect()).unwrap()
    }

    #[test]
    fn rotate_90_has_expected_layout() {
        let image = apply_orientation(fixture(), Orientation::Rotate90);
        let values: Vec<u32> = image.pixels.iter().map(|pixel| pixel[0] as u32).collect();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(values, vec![4, 2, 0, 5, 3, 1]);
    }

    #[test]
    fn rotate_270_has_expected_layout() {
        let image = apply_orientation(fixture(), Orientation::Rotate270);
        let values: Vec<u32> = image.pixels.iter().map(|pixel| pixel[0] as u32).collect();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(values, vec![1, 3, 5, 0, 2, 4]);
    }
}
