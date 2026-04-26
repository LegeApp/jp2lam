use super::{layout, rate, t1, t2};
use crate::dwt::norms::{band_gain, reversible_exponent};
use crate::dwt::pcrd::select_for_quality;
use crate::dwt::{forward_53_2d_in_place, forward_97_2d_in_place};
use crate::encode::backend::CodestreamBackend;
use crate::encode::context::EncodeContext;
use crate::encode::profile_enter;
use crate::error::{Jp2LamError, Result};
use crate::j2k::{build_main_header_segments, CodestreamParts, TilePart, TilePartHeader};
use crate::perceptual::{build_contrast_mask_map_from_luma_u8, ContrastMaskMap, ContrastMaskParams};
use crate::plan::{EncodeLane, EncodingPlan, QuantizationStyle, SubbandQuant, WaveletTransform};

pub(crate) struct NativeBackend;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeComponentCoefficients {
    pub width: usize,
    pub height: usize,
    pub levels: u8,
    pub data: Vec<i32>,
}

impl CodestreamBackend for NativeBackend {
    fn supports(&self, context: &EncodeContext<'_>) -> bool {
        // Enable for testing - GrayLossless lane is under development
        self.supports_lane(context)
    }

    fn encode_codestream(&self, context: &EncodeContext<'_>) -> Result<Vec<u8>> {
        let _p = crate::encode::profile_enter("encode_codestream");
        if !self.supports_lane(context) {
            return Err(Jp2LamError::EncodeFailed(
                "native backend only supports GrayLossless".to_string(),
            ));
        }
        self.prepare_codestream_bytes(context)
    }
}

impl NativeBackend {
    /// Prepare 9/7 irreversible coefficients for a component.
    ///
    /// Pipeline:
    /// 1. DC level-shift (unsigned -> signed-centered).
    /// 2. Optional irreversible MCT for RGB.
    /// 3. Forward 9/7 2-D transform in `f32`.
    /// 4. Per-band scalar-expounded quantization from the plan's QCD metadata.
    /// 4. Return `i32` sign-magnitude coefficients consumable by Tier-1.
    pub(crate) fn prepare_component_coefficients_97(
        &self,
        context: &EncodeContext<'_>,
        component_index: usize,
    ) -> Result<NativeComponentCoefficients> {
        let _p = crate::encode::profile_enter("prepare_component_coefficients_97");
        let mut data = irreversible_input_component(context, component_index)?;

        let width = context.plan.width as usize;
        let height = context.plan.height as usize;
        let levels = context.plan.decomposition_levels;

        forward_97_2d_in_place(&mut data, width, height, levels)?;

        let precision = context
            .plan
            .components
            .get(component_index)
            .map(|component| component.precision)
            .unwrap_or(8);
        let quantized = quantize_97_coefficients(
            &data,
            width,
            height,
            levels,
            precision,
            &context.plan.subband_quants,
        )?;

        Ok(NativeComponentCoefficients {
            width,
            height,
            levels,
            data: quantized,
        })
    }

    pub(super) fn supports_lane(&self, context: &EncodeContext<'_>) -> bool {
        matches!(
            context.plan.lane,
            EncodeLane::GrayLossless
                | EncodeLane::RgbLossless
                | EncodeLane::GrayLossy
                | EncodeLane::RgbLossy
        )
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_component_coefficients(
        &self,
        context: &EncodeContext<'_>,
        component_index: usize,
    ) -> Result<NativeComponentCoefficients> {
        if !self.supports_lane(context) {
            return Err(Jp2LamError::EncodeFailed(
                "native coefficient preparation is not implemented for this lane".to_string(),
            ));
        }

        if matches!(context.plan.transform, WaveletTransform::Irreversible97) {
            return self.prepare_component_coefficients_97(context, component_index);
        }

        let mut data = reversible_input_component(context, component_index)?;

        forward_53_2d_in_place(
            &mut data,
            context.plan.width as usize,
            context.plan.height as usize,
            context.plan.decomposition_levels,
        )?;

        Ok(NativeComponentCoefficients {
            width: context.plan.width as usize,
            height: context.plan.height as usize,
            levels: context.plan.decomposition_levels,
            data,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_component_layout(
        &self,
        context: &EncodeContext<'_>,
        component_index: usize,
    ) -> Result<layout::NativeComponentLayout> {
        let coefficients = self.prepare_component_coefficients(context, component_index)?;
        layout::build_component_layout(&coefficients, context.plan.code_block_size)
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_tier1_layout(
        &self,
        context: &EncodeContext<'_>,
        component_index: usize,
    ) -> Result<t1::NativeTier1Layout> {
        let _p = crate::encode::profile_enter("prepare_tier1_layout");
        let layout = self.prepare_component_layout(context, component_index)?;
        let precision = context
            .plan
            .components
            .get(component_index)
            .map(|c| c.precision)
            .unwrap_or(8);
        let guard_bits = context.plan.guard_bits;
        // For reversible MCT (RCT), Cb and Cr expand to ±255 (9-bit), so components 1 and 2
        // need one extra bitplane of precision.
        // For irreversible MCT (ICT), the channel ranges are different after ICT: Y has larger
        // magnitude range than Cb/Cr, so we use the component's actual precision.
        let effective_precision = if context.plan.use_mct && component_index > 0 {
            if matches!(context.plan.transform, WaveletTransform::Reversible53) {
                precision + 1
            } else {
                // ICT: after forward transform, components are Y (0), Cb (1), Cr (2).
                // Y has wider range than Cb/Cr, but we use base precision for all.
                precision
            }
        } else {
            precision
        };
        let analyzed = match context.plan.quantization_style {
            QuantizationStyle::NoQuantization => {
                t1::analyze_component_layout_with(&layout, effective_precision, guard_bits)
            }
            QuantizationStyle::ScalarExpounded => {
                t1::analyze_component_layout_with_max_bitplanes(&layout, |resolution, band| {
                    context
                        .plan
                        .subband_quants
                        .iter()
                        .find(|quant| quant.resolution == resolution && quant.band == band)
                        .map(|quant| guard_bits.saturating_sub(1).saturating_add(quant.exponent))
                        .unwrap_or_else(|| reversible_exponent(precision, band))
                })
            }
        };
        Ok(analyzed)
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_tier1_encoded_layout(
        &self,
        context: &EncodeContext<'_>,
    ) -> Result<t1::NativeEncodedTier1Layout> {
        let analyzed = self.prepare_tier1_layout(context, 0)?;
        Ok(t1::encode_placeholder_tier1(&analyzed))
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_tier1_encoded_layouts(
        &self,
        context: &EncodeContext<'_>,
    ) -> Result<Vec<t1::NativeEncodedTier1Layout>> {
        let _p = crate::encode::profile_enter("prepare_tier1_encoded_layouts");

        #[cfg(feature = "parallel")]
        let encoded_layouts: Result<Vec<_>> = {
            use rayon::prelude::*;
            (0..(context.plan.component_count as usize))
                .into_par_iter()
                .map(|component_index| {
                    let _cp = crate::encode::profile_enter("per_component_encode");
                    let analyzed = self.prepare_tier1_layout(context, component_index)?;
                    Ok(t1::encode_placeholder_tier1(&analyzed))
                })
                .collect()
        };

        #[cfg(not(feature = "parallel"))]
        let encoded_layouts: Result<Vec<_>> = {
            let mut layouts = Vec::with_capacity(context.plan.component_count as usize);
            for component_index in 0..(context.plan.component_count as usize) {
                let _cp = crate::encode::profile_enter("per_component_encode");
                let analyzed = self.prepare_tier1_layout(context, component_index)?;
                layouts.push(t1::encode_placeholder_tier1(&analyzed));
            }
            Ok(layouts)
        };

        let mut encoded_layouts = encoded_layouts?;
        if native_pcrd_enabled() {
            apply_pcrd_selection(&mut encoded_layouts, context)?;
        }
        Ok(encoded_layouts)
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_packet_sequence(
        &self,
        context: &EncodeContext<'_>,
    ) -> Result<t2::NativePacketSequence> {
        let encoded = self.prepare_tier1_encoded_layouts(context)?;
        t2::build_packet_sequence_for_components(&encoded)
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_tile_part_payload(
        &self,
        context: &EncodeContext<'_>,
    ) -> Result<crate::t2::TilePartPayload> {
        let _p = crate::encode::profile_enter("prepare_tile_part_payload");
        let encoded = self.prepare_tier1_encoded_layouts(context)?;
        t2::build_tile_part_payload_for_components(&encoded)
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_codestream_parts(
        &self,
        context: &EncodeContext<'_>,
    ) -> Result<CodestreamParts> {
        let _p = crate::encode::profile_enter("prepare_codestream_parts");
        if !self.supports_lane(context) {
            return Err(Jp2LamError::EncodeFailed(
                "native codestream assembly is not implemented for this lane".to_string(),
            ));
        }
        let emit_plan = native_emit_plan(&context.plan);
        let payload = self.prepare_tile_part_payload(context)?;
        Ok(CodestreamParts {
            main_header_segments: build_main_header_segments(&emit_plan)?,
            tile_parts: vec![TilePart {
                header: TilePartHeader {
                    tile_index: 0,
                    part_index: 0,
                    total_parts: 1,
                },
                header_segments: Vec::new(),
                payload,
            }],
        })
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_codestream_bytes(&self, context: &EncodeContext<'_>) -> Result<Vec<u8>> {
        let _p = crate::encode::profile_enter("prepare_codestream_bytes");
        let parts = self.prepare_codestream_parts(context)?;
        parts.encode(&native_emit_plan(&context.plan))
    }
}

fn irreversible_input_component(
    context: &EncodeContext<'_>,
    component_index: usize,
) -> Result<Vec<f32>> {
    if !native_use_mct(&context.plan) {
        let source = context.component_data(component_index).ok_or_else(|| {
            Jp2LamError::EncodeFailed(format!("missing component {component_index} samples"))
        })?;
        return Ok(source
            .iter()
            .map(|&sample| (sample - (1 << 7)) as f32)
            .collect());
    }

    if context.plan.component_count != 3 {
        return Err(Jp2LamError::EncodeFailed(
            "irreversible MCT requires exactly 3 components".to_string(),
        ));
    }
    let r = context
        .component_data(0)
        .ok_or_else(|| Jp2LamError::EncodeFailed("missing component 0 samples".to_string()))?;
    let g = context
        .component_data(1)
        .ok_or_else(|| Jp2LamError::EncodeFailed("missing component 1 samples".to_string()))?;
    let b = context
        .component_data(2)
        .ok_or_else(|| Jp2LamError::EncodeFailed("missing component 2 samples".to_string()))?;
    if r.len() != g.len() || r.len() != b.len() {
        return Err(Jp2LamError::EncodeFailed(
            "component sample lengths differ for irreversible MCT".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(r.len());
    for i in 0..r.len() {
        let rf = (r[i] - (1 << 7)) as f32;
        let gf = (g[i] - (1 << 7)) as f32;
        let bf = (b[i] - (1 << 7)) as f32;
        let transformed = match component_index {
            // Y = 0.299*R + 0.587*G + 0.114*B
            // Note: Using explicit * to avoid operator precedence bugs with nested mul_add.
            // The previous mul_add chain for Cb and Cr had parsing issues:
            //   -0.168_75f32.mul_add(rf, -0.331_26f32.mul_add(gf, 0.5 * bf))
            // was parsed as: -0.16875*rf + (-0.33126 * (gf + 0.5*bf))  WRONG!
            // Should be: -0.16875*rf + (-0.33126*gf) + (0.5*bf) = -0.16875*rf - 0.33126*gf + 0.5*bf
            0 => 0.299f32 * rf + 0.587f32 * gf + 0.114f32 * bf,
            // Cb = -0.16875*R - 0.33126*G + 0.5*B
            1 => -0.168_75f32 * rf + -0.331_26f32 * gf + 0.5f32 * bf,
            // Cr = 0.5*R - 0.41869*G - 0.08131*B
            2 => 0.5f32 * rf - 0.418_69f32 * gf - 0.081_31f32 * bf,
            _ => {
                return Err(Jp2LamError::EncodeFailed(format!(
                    "irreversible MCT only supports component index 0..2, got {component_index}"
                )));
            }
        };
        out.push(transformed);
    }
    Ok(out)
}

fn reversible_input_component(
    context: &EncodeContext<'_>,
    component_index: usize,
) -> Result<Vec<i32>> {
    if !native_use_mct(&context.plan) {
        let source = context.component_data(component_index).ok_or_else(|| {
            Jp2LamError::EncodeFailed(format!("missing component {component_index} samples"))
        })?;
        return Ok(source.iter().map(|&s| s - (1 << 7)).collect());
    }
    if context.plan.component_count != 3 {
        return Err(Jp2LamError::EncodeFailed(
            "reversible MCT requires exactly 3 components".to_string(),
        ));
    }
    let r = context
        .component_data(0)
        .ok_or_else(|| Jp2LamError::EncodeFailed("missing component 0 samples".to_string()))?;
    let g = context
        .component_data(1)
        .ok_or_else(|| Jp2LamError::EncodeFailed("missing component 1 samples".to_string()))?;
    let b = context
        .component_data(2)
        .ok_or_else(|| Jp2LamError::EncodeFailed("missing component 2 samples".to_string()))?;
    if r.len() != g.len() || r.len() != b.len() {
        return Err(Jp2LamError::EncodeFailed(
            "component sample lengths differ for reversible MCT".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(r.len());
    for i in 0..r.len() {
        let rv = r[i] - (1 << 7);
        let gv = g[i] - (1 << 7);
        let bv = b[i] - (1 << 7);
        let transformed = match component_index {
            0 => (rv + 2 * gv + bv) >> 2,
            1 => bv - gv,
            2 => rv - gv,
            _ => {
                return Err(Jp2LamError::EncodeFailed(format!(
                    "reversible MCT only supports component index 0..2, got {component_index}"
                )));
            }
        };
        out.push(transformed);
    }
    Ok(out)
}

fn quantize_97_coefficients(
    data: &[f32],
    width: usize,
    height: usize,
    levels: u8,
    precision: u32,
    subband_quants: &[SubbandQuant],
) -> Result<Vec<i32>> {
    let _p = profile_enter("quantize_97_coefficients");
    if data.len() != width.saturating_mul(height) {
        return Err(Jp2LamError::EncodeFailed(
            "irreversible quantization received mismatched coefficient geometry".to_string(),
        ));
    }
    // subband_quants already carry quality-scaled step sizes from the plan.
    let mut out = vec![0i32; data.len()];
    if width == 0 || height == 0 {
        return Ok(out);
    }

    let mut resolutions = Vec::with_capacity(levels as usize + 1);
    let mut rw = width;
    let mut rh = height;
    resolutions.push((rw, rh));
    for _ in 0..levels {
        rw = rw.div_ceil(2);
        rh = rh.div_ceil(2);
        resolutions.push((rw, rh));
    }
    resolutions.reverse();

    let ll = resolutions[0];
    let ll_step = subband_quant_step(
        precision,
        0,
        crate::plan::BandOrientation::Ll,
        subband_quants,
    )?;
    quantize_subband_rect(data, &mut out, width, 0, 0, ll.0, ll.1, ll_step);

    for (index, &[low, full]) in resolutions.array_windows::<2>().enumerate() {
        let resolution = (index + 1) as u8;
        let hl_step = subband_quant_step(
            precision,
            resolution,
            crate::plan::BandOrientation::Hl,
            subband_quants,
        )?;
        let lh_step = subband_quant_step(
            precision,
            resolution,
            crate::plan::BandOrientation::Lh,
            subband_quants,
        )?;
        let hh_step = subband_quant_step(
            precision,
            resolution,
            crate::plan::BandOrientation::Hh,
            subband_quants,
        )?;

        quantize_subband_rect(data, &mut out, width, low.0, 0, full.0, low.1, hl_step);
        quantize_subband_rect(data, &mut out, width, 0, low.1, low.0, full.1, lh_step);
        quantize_subband_rect(data, &mut out, width, low.0, low.1, full.0, full.1, hh_step);
    }

    Ok(out)
}

fn quantize_subband_rect(
    data: &[f32],
    out: &mut [i32],
    stride: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    step: f32,
) {
    for y in y0..y1 {
        let row = y * stride;
        for x in x0..x1 {
            out[row + x] = quantize_f32_to_i32(data[row + x], step);
        }
    }
}

fn subband_quant_step(
    precision: u32,
    resolution: u8,
    band: crate::plan::BandOrientation,
    subband_quants: &[SubbandQuant],
) -> Result<f32> {
    let quant = subband_quants
        .iter()
        .find(|quant| quant.resolution == resolution && quant.band == band)
        .ok_or_else(|| {
            Jp2LamError::EncodeFailed(format!(
                "missing quantization parameters for resolution={resolution} band={band:?}"
            ))
        })?;
    let numbps = (precision + u32::from(band_gain(band))) as i32;
    let exponent = i32::from(quant.exponent);
    let base = 1.0 + f32::from(quant.mantissa) / 2048.0;
    Ok((base * 2f32.powi(numbps - exponent)).max(1e-6))
}

fn quantize_f32_to_i32(v: f32, step: f32) -> i32 {
    if step <= 0.0 || !v.is_finite() {
        return 0;
    }
    let q = (v / step).trunc();
    // Clamp to i32 range; overflow here means a coefficient larger than ~2^31
    // which cannot occur for reasonable inputs.
    q.clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

fn native_pcrd_enabled() -> bool {
    true
}

fn apply_pcrd_selection(
    layouts: &mut [t1::NativeEncodedTier1Layout],
    context: &EncodeContext<'_>,
) -> Result<()> {
    let quality = context.plan.quality;

    if quality >= 100 {
        return Ok(());
    }

    // Calculate pixel count for resolution-aware lambda scaling
    let pixel_count = context.image.width * context.image.height;

    // Build contrast mask map from luma component for perceptual masking
    let contrast_mask = build_luma_contrast_mask(context);

    // Apply quality-based PCRD selection
    for layout in layouts.iter_mut() {
        let precision = context.plan.components.first().map(|c| c.precision).unwrap_or(8);
        let curves = rate::curves_from_tier1_layout(
            layout,
            context.plan.num_resolutions,
            &context.plan.subband_quants,
            precision,
            quality,
            contrast_mask.as_ref(),
        )
        .map_err(|err| Jp2LamError::EncodeFailed(err.to_string()))?;

        let selection = select_for_quality(&curves, quality, pixel_count)
            .map_err(|err| Jp2LamError::EncodeFailed(err.to_string()))?;
        
        let mut selected_passes = vec![0u16; curves.len()];
        for block in selection.selections {
            if let Some(slot) = selected_passes.get_mut(block.block_id) {
                *slot = block.passes;
            } else {
                return Err(Jp2LamError::EncodeFailed(
                    "PCRD block selection index out of range".to_string(),
                ));
            }
        }
        truncate_layout_passes(layout, &selected_passes)?;
    }
    Ok(())
}

/// Build contrast mask map from the luma component.
///
/// For grayscale images, uses the first component directly.
/// For RGB images, computes luma as Y = 0.299*R + 0.587*G + 0.114*B.
fn build_luma_contrast_mask(context: &EncodeContext<'_>) -> Option<ContrastMaskMap> {
    use crate::model::ColorSpace;

    let width = context.image.width as usize;
    let height = context.image.height as usize;

    // Extract luma component
    let luma = match context.image.colorspace {
        ColorSpace::Gray => {
            // Grayscale: use first component directly
            let comp = context.image.components.first()?;
            comp.data
                .iter()
                .map(|&v| v.clamp(0, 255) as u8)
                .collect::<Vec<u8>>()
        }
        ColorSpace::Srgb => {
            // RGB: compute luma as Y = 0.299*R + 0.587*G + 0.114*B
            if context.image.components.len() < 3 {
                return None;
            }
            let r = &context.image.components[0].data;
            let g = &context.image.components[1].data;
            let b = &context.image.components[2].data;

            r.iter()
                .zip(g.iter())
                .zip(b.iter())
                .map(|((&r, &g), &b)| {
                    let y = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
                    y.clamp(0.0, 255.0) as u8
                })
                .collect::<Vec<u8>>()
        }
        _ => return None, // Unsupported colorspace
    };

    if luma.len() != width * height {
        return None;
    }

    let params = ContrastMaskParams::default();
    Some(build_contrast_mask_map_from_luma_u8(
        &luma, width, height, params,
    ))
}

fn tier1_target_body_bytes(plan: &EncodingPlan) -> Option<u32> {
    let target_rate = plan.layers[0].target_rate?;
    if target_rate <= 0.0 {
        return None;
    }
    let sample_count = u64::from(plan.width).saturating_mul(u64::from(plan.height));
    let source_bytes = plan.components.iter().fold(0u64, |acc, component| {
        let bytes_per_sample = u64::from(component.precision.div_ceil(8));
        acc.saturating_add(sample_count.saturating_mul(bytes_per_sample))
    });
    if source_bytes == 0 {
        return None;
    }
    // Keep initial native PCRD conservative: target a larger body budget than
    // the raw rate hint so decoder-acceptance and visual diagnostics remain
    // stable while we incrementally improve slope modeling.
    let conservative_factor = 3.0f64;
    Some(
        ((source_bytes as f64 * conservative_factor / f64::from(target_rate)).round() as u64)
            .clamp(1, u64::from(u32::MAX)) as u32,
    )
}

fn tier1_component_bytes(layout: &t1::NativeEncodedTier1Layout) -> u32 {
    layout
        .bands
        .iter()
        .flat_map(|band| band.blocks.iter())
        .flat_map(|block| block.passes.iter())
        .fold(0u32, |acc, pass| acc.saturating_add(pass.length as u32))
}

fn distribute_component_budgets(component_bytes: &[u32], target_total_bytes: u32) -> Vec<u32> {
    if component_bytes.is_empty() {
        return Vec::new();
    }
    let mut budgets = vec![0u32; component_bytes.len()];
    let mut remaining_target = target_total_bytes;
    let mut remaining_weight = component_bytes.iter().copied().sum::<u32>();

    for (index, &weight) in component_bytes.iter().enumerate() {
        if index + 1 == component_bytes.len() {
            budgets[index] = remaining_target;
            break;
        }
        if weight == 0 || remaining_weight == 0 || remaining_target == 0 {
            remaining_weight = remaining_weight.saturating_sub(weight);
            continue;
        }
        let mut budget = ((u64::from(remaining_target) * u64::from(weight))
            / u64::from(remaining_weight)) as u32;
        if budget == 0 {
            budget = 1;
        }
        budget = budget.min(remaining_target);
        budgets[index] = budget;
        remaining_target = remaining_target.saturating_sub(budget);
        remaining_weight = remaining_weight.saturating_sub(weight);
    }

    budgets
}

fn truncate_layout_passes(
    layout: &mut t1::NativeEncodedTier1Layout,
    selected_passes: &[u16],
) -> Result<()> {
    let expected_block_count = layout
        .bands
        .iter()
        .map(|band| band.blocks.len())
        .sum::<usize>();
    if selected_passes.len() != expected_block_count {
        return Err(Jp2LamError::EncodeFailed(format!(
            "PCRD block count mismatch: expected {expected_block_count}, got {}",
            selected_passes.len()
        )));
    }

    let mut block_id = 0usize;
    for band in &mut layout.bands {
        for block in &mut band.blocks {
            let retain = usize::from(selected_passes[block_id]).min(block.passes.len());
            if retain == 0 {
                block.passes.clear();
                block_id += 1;
                continue;
            }

            let mut retained = block.passes[..retain].to_vec();
            let payload_count = block
                .passes
                .iter()
                .filter(|pass| !pass.bytes.is_empty())
                .count();
            let single_payload = if payload_count == 1 {
                block
                    .passes
                    .iter()
                    .find(|pass| !pass.bytes.is_empty())
                    .map(|pass| pass.bytes.as_slice())
            } else {
                None
            };

            let mut cumulative = 0usize;
            let retained_len = retained.len();
            for (index, pass) in retained.iter_mut().enumerate() {
                cumulative = cumulative.saturating_add(pass.length);
                pass.cumulative_length = cumulative;
                pass.is_terminated = index + 1 == retained_len;
                if let Some(payload) = single_payload {
                    if pass.is_terminated {
                        let prefix_len = cumulative.min(payload.len());
                        pass.bytes = payload[..prefix_len].to_vec();
                    } else {
                        pass.bytes.clear();
                    }
                }
            }

            block.passes = retained;
            block_id += 1;
        }
    }
    Ok(())
}

fn native_emit_plan(plan: &EncodingPlan) -> EncodingPlan {
    let mut adjusted = plan.clone();
    adjusted.use_mct = native_use_mct(plan);
    adjusted
}

fn native_use_mct(plan: &EncodingPlan) -> bool {
    plan.use_mct
}
