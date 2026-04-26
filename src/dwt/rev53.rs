#![allow(dead_code)]

use crate::encode::profile_enter;
use crate::error::{Jp2LamError, Result};

pub(crate) fn forward_53_2d_in_place(
    data: &mut [i32],
    width: usize,
    height: usize,
    levels: u8,
) -> Result<()> {
    let _p = profile_enter("dwt::forward_53_2d_in_place");
    let expected_len = width
        .checked_mul(height)
        .ok_or_else(|| Jp2LamError::EncodeFailed("DWT image dimensions overflow".to_string()))?;
    if data.len() != expected_len {
        return Err(Jp2LamError::EncodeFailed(format!(
            "DWT input length {} did not match image area {expected_len}",
            data.len()
        )));
    }
    if width == 0 || height == 0 || levels == 0 {
        return Ok(());
    }

    let resolutions = encode_resolutions(width, height, levels);
    let max_span = width.max(height);
    let mut scratch = vec![0i32; max_span];

    for &(rw, rh) in resolutions.iter().skip(1).rev() {
        for x in 0..rw {
            gather_column(data, width, rh, x, &mut scratch[..rh]);
            forward_53_1d_in_place(&mut scratch[..rh], true);
            scatter_column(data, width, rh, x, &scratch[..rh]);
        }

        for y in 0..rh {
            let row_start = y * width;
            forward_53_1d_in_place(&mut data[row_start..row_start + rw], true);
        }
    }

    Ok(())
}

fn forward_53_1d_in_place(samples: &mut [i32], even: bool) {
    let width = samples.len();
    if width <= 1 {
        if !even && width == 1 {
            samples[0] *= 2;
        }
        return;
    }

    let sn = (width + if even { 1 } else { 0 }) >> 1;
    let dn = width - sn;
    if even {
        let mut low = Vec::with_capacity(sn);
        let mut high = Vec::with_capacity(dn);
        for (index, &sample) in samples.iter().enumerate() {
            if index.is_multiple_of(2) {
                low.push(sample);
            } else {
                high.push(sample);
            }
        }

        // Reversible 5/3 predict step on odd samples.
        for i in 0..dn {
            let left = low[i];
            let right = low[(i + 1).min(sn - 1)];
            high[i] -= (left + right) >> 1;
        }

        // Reversible 5/3 update step on even samples.
        for i in 0..sn {
            let left = high[i.saturating_sub(1).min(dn - 1)];
            let right = high[i.min(dn - 1)];
            low[i] += (left + right + 2) >> 2;
        }

        samples[..sn].copy_from_slice(&low);
        samples[sn..sn + dn].copy_from_slice(&high);
    } else {
        let mut tmp = vec![0i32; width];
        tmp[sn] = samples[0] - samples[1];
        for i in 1..sn {
            tmp[sn + i] = samples[2 * i] - ((samples[2 * i + 1] + samples[2 * (i - 1) + 1]) >> 1);
        }
        if !width.is_multiple_of(2) {
            let i = sn;
            tmp[sn + i] = samples[2 * i] - samples[2 * (i - 1) + 1];
        }

        for i in 0..dn.saturating_sub(1) {
            samples[i] = samples[2 * i + 1] + ((tmp[sn + i] + tmp[sn + i + 1] + 2) >> 2);
        }
        if width.is_multiple_of(2) {
            let i = dn - 1;
            samples[i] = samples[2 * i + 1] + ((tmp[sn + i] + tmp[sn + i] + 2) >> 2);
        }
        samples[sn..sn + dn].copy_from_slice(&tmp[sn..sn + dn]);
    }
}

fn encode_resolutions(width: usize, height: usize, levels: u8) -> Vec<(usize, usize)> {
    let mut resolutions = Vec::with_capacity(levels as usize + 1);
    let mut w = width;
    let mut h = height;
    resolutions.push((w, h));
    for _ in 0..levels {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        resolutions.push((w, h));
    }
    resolutions.reverse();
    resolutions
}

fn gather_column(data: &[i32], stride: usize, height: usize, x: usize, out: &mut [i32]) {
    for y in 0..height {
        out[y] = data[y * stride + x];
    }
}

fn scatter_column(data: &mut [i32], stride: usize, height: usize, x: usize, values: &[i32]) {
    for y in 0..height {
        data[y * stride + x] = values[y];
    }
}

#[cfg(test)]
mod tests {
    use super::forward_53_2d_in_place;

    #[test]
    fn one_level_transform_matches_known_2x2_case() {
        let mut data = vec![1, 2, 3, 4];
        forward_53_2d_in_place(&mut data, 2, 2, 1).expect("forward dwt");
        assert_eq!(data, vec![3, 1, 2, 0]);
    }

    #[test]
    fn one_level_transform_matches_known_1d_row_case() {
        let mut data = vec![10, 20, 30, 40];
        forward_53_2d_in_place(&mut data, 4, 1, 1).expect("forward row dwt");
        assert_eq!(data, vec![10, 33, 0, 10]);
    }

    #[test]
    fn multi_level_transform_preserves_length_and_runs_on_odd_sizes() {
        let mut data = (0..35).collect::<Vec<_>>();
        forward_53_2d_in_place(&mut data, 5, 7, 2).expect("forward dwt");
        assert_eq!(data.len(), 35);
    }

    #[test]
    fn forward_then_inverse_53_roundtrips_exactly_for_small_images() {
        for height in 1..=8 {
            for width in 1..=8 {
                let levels = max_decompositions(width, height).min(3) as u8;
                for (name, original) in tiny_patterns(width, height) {
                    let mut data = original.clone();
                    forward_53_2d_in_place(&mut data, width, height, levels).expect("forward dwt");
                    inverse_53_2d_in_place_for_test(&mut data, width, height, levels);
                    assert_eq!(
                        data, original,
                        "{name} failed for {width}x{height} with {levels} levels"
                    );
                }
            }
        }
    }

    #[test]
    fn forward_then_inverse_53_roundtrips_exactly_at_5_levels_non_pow2() {
        // These are the actual sizes used by the encoder for RGB lossless test images.
        let cases: &[(usize, usize, u8)] = &[
            (48, 40, 5),
            (64, 48, 5),
            (32, 32, 5),
        ];
        for &(width, height, levels) in cases {
            for (name, original) in tiny_patterns(width, height) {
                let mut data = original.clone();
                forward_53_2d_in_place(&mut data, width, height, levels).expect("forward dwt");
                inverse_53_2d_in_place_for_test(&mut data, width, height, levels);
                assert_eq!(
                    data, original,
                    "{name} failed for {width}x{height} with {levels} levels"
                );
            }
        }
    }

    fn inverse_53_2d_in_place_for_test(data: &mut [i32], width: usize, height: usize, levels: u8) {
        if width == 0 || height == 0 || levels == 0 {
            return;
        }

        let resolutions = encode_resolutions_for_test(width, height, levels);
        let max_span = width.max(height);
        let mut scratch = vec![0i32; max_span];

        for &(rw, rh) in resolutions.iter().skip(1) {
            for y in 0..rh {
                let row_start = y * width;
                inverse_53_1d_even_for_test(&mut data[row_start..row_start + rw]);
            }

            for x in 0..rw {
                for y in 0..rh {
                    scratch[y] = data[y * width + x];
                }
                inverse_53_1d_even_for_test(&mut scratch[..rh]);
                for y in 0..rh {
                    data[y * width + x] = scratch[y];
                }
            }
        }
    }

    fn inverse_53_1d_even_for_test(coefficients: &mut [i32]) {
        let width = coefficients.len();
        if width <= 1 {
            return;
        }

        let sn = width.div_ceil(2);
        let dn = width - sn;
        let low = coefficients[..sn].to_vec();
        let high = coefficients[sn..].to_vec();
        let mut even = vec![0i32; sn];
        let mut out = vec![0i32; width];

        if dn == 0 {
            coefficients.copy_from_slice(&low);
            return;
        }

        even[0] = low[0] - ((high[0] + high[0] + 2) >> 2);
        if sn > 1 {
            for i in 1..sn {
                even[i] = if i < dn {
                    low[i] - ((high[i - 1] + high[i] + 2) >> 2)
                } else {
                    low[i] - ((high[i - 1] + high[i - 1] + 2) >> 2)
                };
            }
        }

        for i in 0..sn {
            out[2 * i] = even[i];
        }
        for i in 0..dn {
            out[2 * i + 1] = if i + 1 < sn {
                high[i] + ((even[i] + even[i + 1]) >> 1)
            } else {
                high[i] + even[i]
            };
        }

        coefficients.copy_from_slice(&out);
    }

    fn encode_resolutions_for_test(width: usize, height: usize, levels: u8) -> Vec<(usize, usize)> {
        let mut resolutions = Vec::with_capacity(levels as usize + 1);
        let mut w = width;
        let mut h = height;
        resolutions.push((w, h));
        for _ in 0..levels {
            w = w.div_ceil(2);
            h = h.div_ceil(2);
            resolutions.push((w, h));
        }
        resolutions.reverse();
        resolutions
    }

    fn max_decompositions(width: usize, height: usize) -> usize {
        let min_dim = width.min(height);
        if min_dim <= 1 {
            return 0;
        }
        usize::BITS as usize - 1 - min_dim.leading_zeros() as usize
    }

    fn tiny_patterns(width: usize, height: usize) -> Vec<(&'static str, Vec<i32>)> {
        let len = width * height;
        let mut patterns = vec![
            ("zeros", vec![0; len]),
            ("ones", vec![1; len]),
            (
                "horizontal_ramp",
                (0..height)
                    .flat_map(|_| (0..width).map(|x| x as i32))
                    .collect(),
            ),
            (
                "vertical_ramp",
                (0..height)
                    .flat_map(|y| (0..width).map(move |_| y as i32))
                    .collect(),
            ),
            (
                "checkerboard",
                (0..height)
                    .flat_map(|y| (0..width).map(move |x| ((x + y) & 1) as i32))
                    .collect(),
            ),
        ];

        for y in 0..height {
            for x in 0..width {
                let mut data = vec![0; len];
                data[y * width + x] = 255;
                patterns.push(("impulse", data));
            }
        }

        patterns
    }
}
