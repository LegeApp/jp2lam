#![allow(dead_code)]

use crate::encode::profile_enter;
use crate::error::{Jp2LamError, Result};

// ISO/IEC 15444-1 Annex F.4.8.2 irreversible 9/7 lifting constants.
const ALPHA: f32 = -1.586_134_342_059_924;
const BETA: f32 = -0.052_980_118_572_961;
const GAMMA: f32 = 0.882_911_075_530_934;
const DELTA: f32 = 0.443_506_852_043_971;
const K: f32 = 1.230_174_104_914_001;
const INV_K: f32 = 1.0 / K;

pub(crate) fn forward_97_2d_in_place(
    data: &mut [f32],
    width: usize,
    height: usize,
    levels: u8,
) -> Result<()> {
    let _p = profile_enter("dwt::forward_97_2d_in_place");
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
    let mut scratch = vec![0f32; max_span];

    for &(rw, rh) in resolutions.iter().skip(1).rev() {
        for x in 0..rw {
            gather_column(data, width, rh, x, &mut scratch[..rh]);
            forward_97_1d_in_place(&mut scratch[..rh]);
            scatter_column(data, width, rh, x, &scratch[..rh]);
        }

        for y in 0..rh {
            let row_start = y * width;
            forward_97_1d_in_place(&mut data[row_start..row_start + rw]);
        }
    }

    Ok(())
}

/// Forward 9/7 lifting on one row, even-origin, with whole-sample symmetric
/// extension at boundaries. Output is deinterleaved: samples[..sn] are the low
/// (s) coefficients, samples[sn..] are the high (d) coefficients.
fn forward_97_1d_in_place(samples: &mut [f32]) {
    let n = samples.len();
    if n < 2 {
        if n == 1 {
            samples[0] *= INV_K;
        }
        return;
    }

    // Step 1: odd positions += ALPHA * (even left + even right).
    apply_lift(samples, 1, ALPHA);
    // Step 2: even positions += BETA * (odd left + odd right).
    apply_lift(samples, 0, BETA);
    // Step 3: odd positions += GAMMA * (even left + even right).
    apply_lift(samples, 1, GAMMA);
    // Step 4: even positions += DELTA * (odd left + odd right).
    apply_lift(samples, 0, DELTA);

    // Step 5: scaling. Low (even) gets 1/K, high (odd) gets K.
    for i in (0..n).step_by(2) {
        samples[i] *= INV_K;
    }
    for i in (1..n).step_by(2) {
        samples[i] *= K;
    }

    // Deinterleave into [low | high].
    let sn = n.div_ceil(2);
    let dn = n - sn;
    let mut tmp = vec![0f32; n];
    for i in 0..sn {
        tmp[i] = samples[2 * i];
    }
    for i in 0..dn {
        tmp[sn + i] = samples[2 * i + 1];
    }
    samples.copy_from_slice(&tmp);
}

fn apply_lift(samples: &mut [f32], start_parity: usize, coeff: f32) {
    let n = samples.len();
    let mut j = start_parity;
    while j < n {
        let left = fetch_sym(samples, j as isize - 1);
        let right = fetch_sym(samples, j as isize + 1);
        samples[j] += coeff * (left + right);
        j += 2;
    }
}

/// Whole-sample symmetric extension: x[-i] = x[i], x[n-1+i] = x[n-1-i].
#[inline(always)]
fn fetch_sym(samples: &[f32], i: isize) -> f32 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return samples[0];
    }

    // Fast path: in-bounds access (most common case in DWT lifting)
    if i >= 0 && i < n as isize {
        // SAFETY: bounds checked above
        return unsafe { *samples.get_unchecked(i as usize) };
    }

    // Slow path: symmetric extension for out-of-bounds indices
    let period = 2 * (n as isize - 1);
    let k = i.rem_euclid(period);
    let idx = if k >= n as isize { (period - k) as usize } else { k as usize };
    samples[idx]
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

fn gather_column(data: &[f32], stride: usize, height: usize, x: usize, out: &mut [f32]) {
    for y in 0..height {
        out[y] = data[y * stride + x];
    }
}

fn scatter_column(data: &mut [f32], stride: usize, height: usize, x: usize, values: &[f32]) {
    for y in 0..height {
        data[y * stride + x] = values[y];
    }
}

/// Inverse 9/7 2-D lifting, in-place. Undoes `forward_97_2d_in_place`.
///
/// `data` must be in the deinterleaved JPEG 2000 subband layout produced by the
/// forward transform. After this call, `data` holds the reconstructed signal.
pub(crate) fn inverse_97_2d_in_place(data: &mut [f32], width: usize, height: usize, levels: u8) {
    if width == 0 || height == 0 || levels == 0 {
        return;
    }
    let resolutions = encode_resolutions(width, height, levels);
    let max_span = width.max(height);
    let mut scratch = vec![0f32; max_span];

    for &(rw, rh) in resolutions.iter().skip(1) {
        for y in 0..rh {
            let row_start = y * width;
            inverse_97_1d_in_place(&mut data[row_start..row_start + rw]);
        }
        for x in 0..rw {
            gather_column(data, width, rh, x, &mut scratch[..rh]);
            inverse_97_1d_in_place(&mut scratch[..rh]);
            scatter_column(data, width, rh, x, &scratch[..rh]);
        }
    }
}

fn inverse_97_1d_in_place(samples: &mut [f32]) {
    let n = samples.len();
    if n < 2 {
        if n == 1 {
            samples[0] *= K;
        }
        return;
    }

    let sn = n.div_ceil(2);
    let dn = n - sn;
    let mut inter = vec![0f32; n];
    for i in 0..sn {
        inter[2 * i] = samples[i];
    }
    for i in 0..dn {
        inter[2 * i + 1] = samples[sn + i];
    }

    for i in (0..n).step_by(2) {
        inter[i] *= K;
    }
    for i in (1..n).step_by(2) {
        inter[i] *= INV_K;
    }

    apply_lift(&mut inter, 0, -DELTA);
    apply_lift(&mut inter, 1, -GAMMA);
    apply_lift(&mut inter, 0, -BETA);
    apply_lift(&mut inter, 1, -ALPHA);

    samples.copy_from_slice(&inter);
}

#[cfg(test)]
mod tests {
    use super::{forward_97_1d_in_place, forward_97_2d_in_place, inverse_97_2d_in_place, INV_K};

    const ROUNDTRIP_TOL: f32 = 1e-3;

    #[test]
    fn forward_then_inverse_97_roundtrips_within_tolerance_for_small_images() {
        for height in 1..=8 {
            for width in 1..=8 {
                let levels = max_decompositions(width, height).min(3) as u8;
                for (name, original) in tiny_patterns(width, height) {
                    let mut data = original.iter().map(|&v| v as f32).collect::<Vec<_>>();
                    forward_97_2d_in_place(&mut data, width, height, levels).expect("forward dwt");
                    inverse_97_2d_in_place(&mut data, width, height, levels);
                    for (idx, (&out, &orig)) in data.iter().zip(original.iter()).enumerate() {
                        let diff = (out - orig as f32).abs();
                        assert!(
                            diff < ROUNDTRIP_TOL,
                            "{name} {width}x{height} levels={levels} idx={idx} diff={diff} out={out} orig={orig}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn constant_signal_concentrates_into_single_ll_coefficient() {
        // Per Annex F, the low-pass filter of the 9/7 is normalized so that a
        // DC input survives only in LL. After N levels, only position (0,0)
        // should be non-zero; all detail bands should be near zero.
        let (w, h) = (8usize, 8usize);
        let mut data = vec![128.0f32; w * h];
        forward_97_2d_in_place(&mut data, w, h, 3).expect("forward dwt");
        for y in 0..h {
            for x in 0..w {
                if x == 0 && y == 0 {
                    continue;
                }
                let v = data[y * w + x].abs();
                assert!(v < 1e-2, "detail coefficient at ({x},{y}) = {v}");
            }
        }
        assert!(data[0].abs() > 1.0, "LL coefficient should be non-trivial");
    }

    #[test]
    fn length_one_signal_is_scaled_by_inv_k() {
        let mut row = [100.0f32];
        forward_97_1d_in_place(&mut row);
        assert!((row[0] - 100.0 * INV_K).abs() < 1e-5);
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
