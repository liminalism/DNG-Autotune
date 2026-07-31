use crate::types::LinearImage;
use rawler::Orientation;

pub fn apply_orientation(mut image: LinearImage, orientation: Orientation) -> LinearImage {
    let (transpose, horizontal, vertical) = orientation.to_flips();

    if horizontal {
        for row in image.pixels.chunks_exact_mut(image.width) {
            row.reverse();
        }
    }

    if vertical {
        let half = image.height / 2;
        for y in 0..half {
            let opposite = image.height - 1 - y;
            let row_a = y * image.width;
            let row_b = opposite * image.width;
            for x in 0..image.width {
                image.pixels.swap(row_a + x, row_b + x);
            }
        }
    }

    if transpose {
        let old_width = image.width;
        let old_height = image.height;
        let mut transposed = vec![[0.0_f32; 3]; image.pixels.len()];

        for y in 0..old_height {
            for x in 0..old_width {
                let source = y * old_width + x;
                let destination = x * old_height + y;
                transposed[destination] = image.pixels[source];
            }
        }

        image.width = old_height;
        image.height = old_width;
        image.pixels = transposed;
    }

    image
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
