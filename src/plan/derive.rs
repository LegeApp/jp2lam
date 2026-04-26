use crate::dwt::norms::{irreversible_expounded_quant, reversible_exponent};
use crate::model::{ColorSpace, Image, Preset};

use super::{
    BandOrientation, ComponentPlan, EncodeLane, QuantizationStyle, SubbandQuant, WaveletTransform,
};

/// Parameters derived from a [`Preset`].
#[derive(Debug, Clone, Copy)]
pub(super) struct PresetParams {
    pub quality: u8,
    pub transform: WaveletTransform,
    pub use_mct: bool,
}

impl Preset {
    pub(super) fn params(self) -> PresetParams {
        match self {
            Preset::DocumentLow => PresetParams {
                quality: 30,
                transform: WaveletTransform::Irreversible97,
                use_mct: true,
            },
            Preset::DocumentHigh => PresetParams {
                quality: 85,
                transform: WaveletTransform::Irreversible97,
                use_mct: true,
            },
            Preset::WebLow => PresetParams {
                quality: 42,
                transform: WaveletTransform::Irreversible97,
                use_mct: true,
            },
            Preset::WebHigh => PresetParams {
                quality: 62,
                transform: WaveletTransform::Irreversible97,
                use_mct: true,
            },
        }
    }
}

pub(super) fn derive_lane(color_space: ColorSpace, is_lossless: bool) -> EncodeLane {
    match (color_space.encoding_domain(), is_lossless) {
        (ColorSpace::Gray, true) => EncodeLane::GrayLossless,
        (ColorSpace::Gray, false) => EncodeLane::GrayLossy,
        (ColorSpace::Srgb, true) => EncodeLane::RgbLossless,
        (ColorSpace::Srgb, false) => EncodeLane::RgbLossy,
        _ => EncodeLane::RgbLossy,
    }
}

pub(super) fn derive_component_plans(image: &Image) -> Vec<ComponentPlan> {
    image.components.iter().map(ComponentPlan::from).collect()
}

pub(super) fn derive_subband_quants(
    precision: u32,
    decomposition_levels: u8,
    transform: WaveletTransform,
) -> (QuantizationStyle, Vec<SubbandQuant>) {
    let style = match transform {
        WaveletTransform::Reversible53 => QuantizationStyle::NoQuantization,
        WaveletTransform::Irreversible97 => QuantizationStyle::ScalarExpounded,
    };

    let num_resolutions = decomposition_levels.saturating_add(1);
    let mut bands = Vec::with_capacity(1 + (decomposition_levels as usize * 3));
    let mut push_band = |resolution: u8, band: BandOrientation| {
        let (exponent, mantissa) =
            band_stepsize(precision, num_resolutions, resolution, band, transform);
        bands.push(SubbandQuant {
            resolution,
            band,
            exponent,
            mantissa,
        });
    };

    push_band(0, BandOrientation::Ll);
    for resolution in 1..=decomposition_levels {
        push_band(resolution, BandOrientation::Hl);
        push_band(resolution, BandOrientation::Lh);
        push_band(resolution, BandOrientation::Hh);
    }
    (style, bands)
}

/// Apply quality-based quantization step scaling to subband quants for quality < 50.
///
/// Increases step size (coarser quantization) so tier-1 bitplane analysis,
/// PCRD distortion estimates, the QCD marker, and the quantizer itself all
/// agree on the actual step sizes used to encode the stream.
pub(super) fn apply_quality_step_scaling(quants: &mut Vec<SubbandQuant>, quality: u8) {
    if quality >= 50 {
        return;
    }
    let scale = quality_step_scaler(quality);
    for sq in quants.iter_mut() {
        *sq = scale_one_quant(sq, scale);
    }
}

/// Returns a step-size multiplier > 1 for quality < 50, 1.0 otherwise.
///
/// Calibrated so q=42→1.32×, q=30→2.0×, q=10→4.0×, q=1→~5.3×.
fn quality_step_scaler(quality: u8) -> f64 {
    if quality >= 50 {
        return 1.0;
    }
    if quality == 0 {
        return 8.0;
    }
    2.0_f64.powf((50.0 - quality as f64) / 20.0)
}

/// Multiply the quantization step of one subband by `scale`.
///
/// Step = (1 + mantissa/2048) * 2^(numbps - exponent).
/// Multiplying by f means decreasing exponent by floor(log2(f)) and adjusting
/// the mantissa for the remaining fractional factor.  The numbps term (which
/// encodes band gain and precision) cancels and is never needed.
fn scale_one_quant(sq: &SubbandQuant, scale: f64) -> SubbandQuant {
    let base = 1.0 + sq.mantissa as f64 / 2048.0;
    let scaled = (base * scale).max(1.0);
    let k = scaled.log2().floor() as i32;
    let base_new = scaled / 2f64.powi(k);
    let exp_new = (sq.exponent as i32 - k).max(0) as u8;
    let mantissa_new = ((base_new - 1.0) * 2048.0).round().clamp(0.0, 2047.0) as u16;
    SubbandQuant {
        resolution: sq.resolution,
        band: sq.band,
        exponent: exp_new,
        mantissa: mantissa_new,
    }
}

pub(super) fn max_target_decompositions(color_space: ColorSpace, preset: Preset) -> u32 {
    match color_space.encoding_domain() {
        ColorSpace::Gray => 5,
        ColorSpace::Srgb => 6,
        _ => 6,
    }
}

pub(super) fn transform_for(preset: Preset) -> WaveletTransform {
    preset.params().transform
}

pub(super) fn use_mct(color_space: ColorSpace, preset: Preset) -> bool {
    match color_space.encoding_domain() {
        ColorSpace::Gray => false,
        _ => preset.params().use_mct,
    }
}

/// Maximum useful decomposition levels for an image of the given dimensions.
///
/// For images ≥ 32px in min dimension, caps levels so the LL subband is
/// at least 16×16, avoiding diminishing returns from micro-subbands.
/// Formula: `min(6, 1 + floor(log2(min_dim / 32)))` for min_dim ≥ 32.
/// For smaller images, falls back to `floor(log2(min_dim))` to match the
/// DWT feasibility limit (LL ≥ 1×1).
pub(super) fn max_decompositions(width: u32, height: u32) -> u32 {
    let min_dim = width.min(height);
    if min_dim <= 1 {
        return 0;
    }
    if min_dim < 32 {
        return (u32::BITS - 1) - min_dim.leading_zeros();
    }
    // Keep LL ≥ 16×16: 1 + floor(log2(min_dim / 32)), capped at 6.
    let ratio = min_dim / 32;
    (1 + (u32::BITS - 1) - ratio.leading_zeros()).min(6)
}

pub(super) fn tcp_rate_from_quality(quality: u8, preset: Preset) -> f32 {
    let q = quality.min(99) as f32;
    let t = (q + 0.5) / 99.5;
    const RATE_HIGH_COMPRESSION: f32 = 42.0;
    const RATE_NEAR_LOSSLESS: f32 = 0.35;
    let mut rate = RATE_HIGH_COMPRESSION * (1.0 - t) + RATE_NEAR_LOSSLESS * t;
    rate *= preset_rate_multiplier(preset);
    rate.min(RATE_HIGH_COMPRESSION * 1.35)
        .max(RATE_NEAR_LOSSLESS)
}

fn preset_rate_multiplier(preset: Preset) -> f32 {
    match preset {
        Preset::DocumentLow => 0.86,
        Preset::DocumentHigh => 0.98,
        Preset::WebLow => 0.88,
        Preset::WebHigh => 1.0,
    }
}

fn band_stepsize(
    precision: u32,
    num_resolutions: u8,
    resolution: u8,
    band: BandOrientation,
    transform: WaveletTransform,
) -> (u8, u16) {
    match transform {
        WaveletTransform::Reversible53 => (reversible_exponent(precision, band), 0),
        WaveletTransform::Irreversible97 => {
            irreversible_expounded_quant(precision, num_resolutions, resolution, band)
        }
    }
}
