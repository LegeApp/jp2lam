#![allow(dead_code)]

//! PCRD adapters that turn native Tier-1 output into rate-distortion curves.
//!
//! This layer keeps the PCRD module itself (`crate::dwt::pcrd`) independent of
//! any Tier-1 data shape. It is the narrow glue point between
//! `NativeEncodedTier1Layout` and `CodeBlockPcrdCurve`.

use crate::dwt::norms::{band_gain, get_norm_97};
use crate::dwt::pcrd::{
    build_hull_curve, estimate_pass_distortion_delta_with_model, BandKind, CodeBlockPcrdCurve,
    DistortionModel, PassDistortionContext, PassKind, PcrdError, RawPassRecord,
};
use crate::plan::{BandOrientation, SubbandQuant};

use super::t1::{NativeEncodedTier1CodeBlock, NativeEncodedTier1Layout};

/// Estimated packet-header signaling cost per included code-block (bytes).
///
/// A single-layer, single-termination encode pays roughly 2–4 bytes of overhead
/// per block for inclusion tag-tree bits, zero-bitplane tag-tree bits, pass-count
/// comma-code, and segment-length field. Adding this to cumulative_length means
/// the slope from "omit block" to "include first pass" reflects the real cost,
/// without affecting the marginal slope between consecutive included passes.
const HEADER_OVERHEAD_BYTES: u32 = 2;

const ACTIVE_DISTORTION_MODEL: DistortionModel = DistortionModel::PassKindAware;

/// Build pruned PCRD hull curves for every code-block in a Tier-1 layout.
///
/// `num_resolutions` is the number of DWT resolutions in the component.
/// `subband_quants` and `precision` supply the quantization step Δ for each
/// subband, making distortion estimates dimensionally consistent with the actual
/// coefficient-domain magnitudes.
/// `quality` enables quality-dependent distortion weighting.
/// `contrast_mask` provides perceptual masking weights based on local texture.
pub(crate) fn curves_from_tier1_layout(
    layout: &NativeEncodedTier1Layout,
    num_resolutions: u8,
    subband_quants: &[SubbandQuant],
    precision: u32,
    quality: u8,
    contrast_mask: Option<&crate::perceptual::ContrastMaskMap>,
) -> Result<Vec<CodeBlockPcrdCurve>, PcrdError> {
    let mut curves = Vec::new();
    let mut next_id = 0usize;

    for band in &layout.bands {
        let weight = subband_weight_for(num_resolutions, band.resolution, band.band);
        let quant_step = subband_quants
            .iter()
            .find(|sq| sq.resolution == band.resolution && sq.band == band.band)
            .map(|sq| quant_step_from_subband(*sq, precision))
            .unwrap_or(1.0);
        let band_kind = band_kind_for(band.band);
        for block in &band.blocks {
            let contrast_weight = compute_block_contrast_weight(
                contrast_mask,
                block,
                band.resolution,
                band.band,
                num_resolutions,
            );
            let raws = raw_records_for_block(
                block,
                weight,
                quant_step,
                band_kind,
                band.resolution,
                quality,
                contrast_weight,
            );
            curves.push(build_hull_curve(next_id, &raws)?);
            next_id += 1;
        }
    }

    Ok(curves)
}

fn raw_records_for_block(
    block: &NativeEncodedTier1CodeBlock,
    subband_weight: f64,
    quant_step: f64,
    band_kind: BandKind,
    resolution: u8,
    quality: u8,
    contrast_visibility_weight: f64,
) -> Vec<RawPassRecord> {
    use crate::encode::backend::native::t1::NativeTier1PassKind;

    let mut prev_cumulative = 0u32;
    let mut records = Vec::with_capacity(block.passes.len());

    for pass in &block.passes {
        let (pass_kind, newly_significant, refinement_samples) = match pass.kind {
            NativeTier1PassKind::MagnitudeRefinement => {
                (PassKind::MagnitudeRefinement, 0, pass.significant_before)
            }
            NativeTier1PassKind::Cleanup => {
                (PassKind::Cleanup, pass.newly_significant, 0)
            }
            _ => (PassKind::SignificancePropagation, pass.newly_significant, 0),
        };

        let ctx = PassDistortionContext {
            pass_kind,
            bitplane: pass.bitplane,
            newly_significant,
            refinement_samples,
            subband_weight,
            quant_step,
            band_kind,
            quality,
            block_class: block.block_class,
            contrast_visibility_weight,
        };
        let distortion_delta =
            estimate_pass_distortion_delta_with_model(&ctx, ACTIVE_DISTORTION_MODEL);

        // Offset cumulative by the header overhead so the "omit → first pass"
        // slope accounts for the per-block packet signaling cost. Each subsequent
        // pass's incremental bytes are unaffected because we track prev_cumulative.
        let cumulative = pass.cumulative_length as u32 + HEADER_OVERHEAD_BYTES;
        let incremental = cumulative - prev_cumulative;
        prev_cumulative = cumulative;
        records.push(RawPassRecord::new(
            pass.pass_index,
            incremental,
            cumulative,
            distortion_delta,
        ));
    }

    records
}

/// Compute contrast visibility weight for a code-block.
///
/// Maps the code-block's wavelet-domain position to the original image space
/// and averages the contrast mask over that region.
fn compute_block_contrast_weight(
    contrast_mask: Option<&crate::perceptual::ContrastMaskMap>,
    block: &NativeEncodedTier1CodeBlock,
    resolution: u8,
    band: BandOrientation,
    num_resolutions: u8,
) -> f64 {
    use crate::perceptual::{average_mask_for_source_rect, SourceRect};
    use crate::plan::BandOrientation;

    let Some(mask) = contrast_mask else {
        return 1.0; // No masking if mask not provided
    };

    // Compute decomposition level: how many times this subband was downsampled
    let decomposition_level = if matches!(band, BandOrientation::Ll) {
        // LL band is at the coarsest level
        num_resolutions.saturating_sub(1)
    } else {
        // High-pass bands: level = (num_resolutions - 1) - resolution
        num_resolutions.saturating_sub(1).saturating_sub(resolution)
    };

    // Source scale: each wavelet level doubles the spatial extent
    let source_scale = 1 << decomposition_level;

    // Map code-block coordinates to source image space
    let x0 = block.x0 as usize * source_scale;
    let y0 = block.y0 as usize * source_scale;
    let x1 = block.x1 as usize * source_scale;
    let y1 = block.y1 as usize * source_scale;

    let rect = SourceRect { x0, y0, x1, y1 };

    average_mask_for_source_rect(mask, rect)
}

fn band_kind_for(band: BandOrientation) -> BandKind {
    match band {
        BandOrientation::Ll => BandKind::Ll,
        BandOrientation::Hl => BandKind::Hl,
        BandOrientation::Lh => BandKind::Lh,
        BandOrientation::Hh => BandKind::Hh,
    }
}

fn subband_weight_for(num_resolutions: u8, resolution: u8, band: BandOrientation) -> f64 {
    // 9/7 synthesis norm squared: contribution of a unit coefficient in this
    // subband to the reconstructed image's squared-error budget.
    let level = match band {
        BandOrientation::Ll => num_resolutions.saturating_sub(1),
        _ => num_resolutions.saturating_sub(1).saturating_sub(resolution),
    };
    let norm = get_norm_97(u32::from(level), band);
    norm * norm
}

/// Decode the quantization step Δ from a packed `SubbandQuant`.
///
/// JPEG 2000 scalar-expounded step: Δ = (1 + μ/2048) · 2^(numbps − ε)
/// where numbps = precision + band_gain, ε = exponent, μ = mantissa.
/// For reversible 5/3 encoding, mantissa = 0 and exponent = numbps, giving Δ = 1.
fn quant_step_from_subband(sq: SubbandQuant, precision: u32) -> f64 {
    let numbps = precision + u32::from(band_gain(sq.band));
    let mantissa_frac = f64::from(sq.mantissa) / 2048.0;
    (1.0 + mantissa_frac) * (2.0f64).powi(numbps as i32 - i32::from(sq.exponent))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::encode::backend::native::NativeBackend;
    use crate::encode::context::EncodeContext;
    use crate::model::{ColorSpace, Component, EncodeOptions, Image, OutputFormat, Preset};

    fn tiny_gray_ctx() -> (Image, EncodeOptions) {
        let image = Image {
            width: 4,
            height: 4,
            components: vec![Component {
                data: vec![
                    0, 16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240,
                ],
                width: 4,
                height: 4,
                precision: 8,
                signed: false,
                dx: 1,
                dy: 1,
            }],
            colorspace: ColorSpace::Gray,
        };
        let options = EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        };
        (image, options)
    }

    #[test]
    fn curves_from_tier1_layout_produce_hulls_with_monotone_slopes() {
        let (image, options) = tiny_gray_ctx();
        let context = EncodeContext::new(&image, &options).expect("build context");
        let encoded = NativeBackend
            .prepare_tier1_encoded_layout(&context)
            .expect("tier1 encoded layout");
        let num_resolutions = context.plan.num_resolutions;

        let precision = context.plan.components.first().map(|c| c.precision).unwrap_or(8);
        let curves = curves_from_tier1_layout(
            &encoded,
            num_resolutions,
            &context.plan.subband_quants,
            precision,
            context.plan.quality,
            None, // No contrast masking in test
        )
        .expect("curves");
        assert!(!curves.is_empty(), "expected at least one block");

        for curve in &curves {
            // Origin point first.
            assert_eq!(curve.points[0].passes, 0);
            assert_eq!(curve.points[0].bytes, 0);
            // Strictly decreasing slopes after origin (monotone convex hull).
            for pair in curve.points.windows(2).skip(1) {
                assert!(
                    pair[1].slope < pair[0].slope,
                    "non-monotone slope in block {}: {} -> {}",
                    curve.block_id,
                    pair[0].slope,
                    pair[1].slope
                );
            }
        }
    }
}
