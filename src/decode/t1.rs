//! Tier-1 code-block decoding for the default Part 1 MQ path.
//!
//! This mirrors the encoder pass scan in `encode/backend/native/t1.rs`:
//! top bit-plane cleanup only, then SP -> MR -> cleanup for lower bit-planes.

use crate::error::{Jp2LamError, Result};
use crate::j2k::decode_markers::CodestreamHeader;
use crate::mq::{MqDecoder, T1_CTXNO_AGG, T1_CTXNO_UNI, T1_CTXNO_ZC};
use crate::plan::BandOrientation;
use crate::tier1::flags::FlagGrid;
use crate::tier1::helpers::{
    magnitude_context, sign_context, sign_prediction_bit, zero_coding_context,
};

use super::t2::DecodedTilePackets;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedCodeBlockCoefficients {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) coefficients: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedTileCoefficients {
    pub(crate) component: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) coefficients: Vec<i32>,
}

pub(crate) fn decode_tile_components(
    header: &CodestreamHeader,
    packets: &DecodedTilePackets<'_>,
) -> Result<Vec<DecodedTileCoefficients>> {
    let width = usize::try_from(header.siz.width)
        .map_err(|_| invalid("tile width exceeds usize"))?;
    let height = usize::try_from(header.siz.height)
        .map_err(|_| invalid("tile height exceeds usize"))?;
    let component_count = header.siz.components.len();
    let mut components = (0..component_count)
        .map(|component| DecodedTileCoefficients {
            component,
            width,
            height,
            coefficients: vec![0i32; width * height],
        })
        .collect::<Vec<_>>();

    for block in &packets.codeblocks {
        let quant = header
            .qcd
            .steps
            .get(block.band_index % header.qcd.steps.len())
            .ok_or_else(|| invalid("missing QCD step for decoded subband"))?;
        let max_bitplanes = header.qcd.guard_bits.saturating_sub(1) + quant.exponent;
        let decoded = decode_codeblock(
            (block.x1 - block.x0) as usize,
            (block.y1 - block.y0) as usize,
            block.band,
            max_bitplanes,
            block.zero_bitplanes,
            block.passes,
            block.data,
        )?;
        let component = components
            .get_mut(block.component)
            .ok_or_else(|| invalid("decoded code-block references invalid component"))?;
        copy_block_to_tile(width, &mut component.coefficients, block.x0, block.y0, &decoded)?;
    }

    Ok(components)
}

#[allow(dead_code)]
pub(crate) fn decode_tile_coefficients(
    header: &CodestreamHeader,
    packets: &DecodedTilePackets<'_>,
) -> Result<DecodedTileCoefficients> {
    let mut components = decode_tile_components(header, packets)?;
    if components.len() != 1 {
        return Err(invalid("single-component decode requested for multi-component tile"));
    }
    Ok(components.remove(0))
}

pub(crate) fn decode_codeblock(
    width: usize,
    height: usize,
    band: BandOrientation,
    max_bitplanes: u8,
    zero_bitplanes: u32,
    pass_count: u32,
    data: &[u8],
) -> Result<DecodedCodeBlockCoefficients> {
    let coefficient_count = width
        .checked_mul(height)
        .ok_or_else(|| invalid("code-block coefficient count overflow"))?;
    let mut magnitudes = vec![0u32; coefficient_count];
    let mut signs = vec![0u8; coefficient_count];

    if pass_count == 0 {
        return Ok(DecodedCodeBlockCoefficients {
            width,
            height,
            coefficients: vec![0; coefficient_count],
        });
    }
    if data.is_empty() {
        return Err(invalid("non-empty code-block has no compressed bytes"));
    }

    let zero_bitplanes = u8::try_from(zero_bitplanes)
        .map_err(|_| invalid("zero-bitplane count exceeds supported range"))?;
    let magnitude_bitplanes = max_bitplanes
        .checked_sub(zero_bitplanes)
        .ok_or_else(|| invalid("zero-bitplane count exceeds subband bit-plane count"))?;
    if magnitude_bitplanes == 0 {
        return Err(invalid("non-empty code-block has no magnitude bit-planes"));
    }
    let max_passes = 1u32 + 3u32 * u32::from(magnitude_bitplanes.saturating_sub(1));
    if pass_count > max_passes {
        return Err(invalid(format!(
            "code-block pass count {pass_count} exceeds maximum {max_passes} for {magnitude_bitplanes} magnitude bit-planes"
        )));
    }

    let mut decoder = MqDecoder::new(data);
    let mut flags = FlagGrid::new(width, height);
    let mut remaining = pass_count;
    let top = magnitude_bitplanes - 1;

    cleanup_decode(
        &mut decoder,
        band,
        top,
        &mut flags,
        &mut magnitudes,
        &mut signs,
        width,
        height,
    );
    remaining -= 1;
    clear_visited_all(&mut flags, width, height);

    for bitplane in (0..top).rev() {
        if remaining == 0 {
            break;
        }
        sigpass_decode(
            &mut decoder,
            band,
            bitplane,
            &mut flags,
            &mut magnitudes,
            &mut signs,
            width,
            height,
        );
        remaining -= 1;

        if remaining == 0 {
            break;
        }
        refpass_decode(
            &mut decoder,
            bitplane,
            &mut flags,
            &mut magnitudes,
            width,
            height,
        );
        remaining -= 1;

        if remaining == 0 {
            break;
        }
        cleanup_decode(
            &mut decoder,
            band,
            bitplane,
            &mut flags,
            &mut magnitudes,
            &mut signs,
            width,
            height,
        );
        remaining -= 1;
        clear_visited_all(&mut flags, width, height);
    }

    let coefficients = magnitudes
        .into_iter()
        .zip(signs)
        .map(|(mag, sign)| {
            if sign == 0 {
                mag as i32
            } else {
                -(mag as i32)
            }
        })
        .collect();

    Ok(DecodedCodeBlockCoefficients {
        width,
        height,
        coefficients,
    })
}

fn sigpass_decode(
    decoder: &mut MqDecoder<'_>,
    band: BandOrientation,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    signs: &mut [u8],
    width: usize,
    height: usize,
) {
    let full_stripes = height / 4;
    let rem = height % 4;

    for k in (0..full_stripes * 4).step_by(4) {
        for x in 0..width {
            for ci in 0..4 {
                sigpass_step(decoder, band, bitplane, flags, magnitudes, signs, width, x, k + ci);
            }
        }
    }
    if rem > 0 {
        let k = full_stripes * 4;
        for x in 0..width {
            for ci in 0..rem {
                sigpass_step(decoder, band, bitplane, flags, magnitudes, signs, width, x, k + ci);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sigpass_step(
    decoder: &mut MqDecoder<'_>,
    band: BandOrientation,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    signs: &mut [u8],
    width: usize,
    x: usize,
    y: usize,
) {
    if flags.is_significant(x, y) {
        return;
    }
    let neighbour_mask = flags.neighbour_mask(x, y);
    if neighbour_mask == 0 {
        return;
    }
    let ctx = zero_coding_context(band, neighbour_mask);
    let plane_bit = decoder.decode_with_ctx(ctx);
    flags.mark_visited(x, y);
    if plane_bit != 0 {
        decode_new_significant(decoder, bitplane, flags, magnitudes, signs, width, x, y);
    }
}

fn refpass_decode(
    decoder: &mut MqDecoder<'_>,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    width: usize,
    height: usize,
) {
    let full_stripes = height / 4;
    let rem = height % 4;

    for k in (0..full_stripes * 4).step_by(4) {
        for x in 0..width {
            for ci in 0..4 {
                refpass_step(decoder, bitplane, flags, magnitudes, width, x, k + ci);
            }
        }
    }
    if rem > 0 {
        let k = full_stripes * 4;
        for x in 0..width {
            for ci in 0..rem {
                refpass_step(decoder, bitplane, flags, magnitudes, width, x, k + ci);
            }
        }
    }
}

fn refpass_step(
    decoder: &mut MqDecoder<'_>,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    width: usize,
    x: usize,
    y: usize,
) {
    if !flags.is_significant(x, y) || flags.is_visited(x, y) {
        return;
    }
    let has_sig_neighbor = flags.neighbour_mask(x, y) != 0;
    let ctx = magnitude_context(has_sig_neighbor, flags.has_refinement_history(x, y));
    let plane_bit = decoder.decode_with_ctx(ctx);
    if plane_bit != 0 {
        magnitudes[y * width + x] |= 1u32 << bitplane;
    }
    flags.mark_refined(x, y);
}

fn cleanup_decode(
    decoder: &mut MqDecoder<'_>,
    band: BandOrientation,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    signs: &mut [u8],
    width: usize,
    height: usize,
) {
    let full_stripes = height / 4;
    let rem = height % 4;

    for k in (0..full_stripes * 4).step_by(4) {
        for x in 0..width {
            cleanup_stripe(
                decoder, band, bitplane, flags, magnitudes, signs, width, x, k, 4,
            );
        }
    }
    if rem > 0 {
        let k = full_stripes * 4;
        for x in 0..width {
            for ci in 0..rem {
                cleanup_sample_regular(
                    decoder,
                    band,
                    bitplane,
                    flags,
                    magnitudes,
                    signs,
                    width,
                    x,
                    k + ci,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cleanup_stripe(
    decoder: &mut MqDecoder<'_>,
    band: BandOrientation,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    signs: &mut [u8],
    width: usize,
    x: usize,
    k: usize,
    lim: usize,
) {
    debug_assert_eq!(lim, 4);
    if flags.stripe_is_clean(x, k, lim) {
        if decoder.decode_with_ctx(T1_CTXNO_AGG) == 0 {
            for ci in 0..lim {
                flags.clear_visited(x, k + ci);
            }
            return;
        }

        let hi = decoder.decode_with_ctx(T1_CTXNO_UNI);
        let lo = decoder.decode_with_ctx(T1_CTXNO_UNI);
        let runlen = ((hi << 1) | lo) as usize;
        let y = k + runlen;
        decode_new_significant(decoder, bitplane, flags, magnitudes, signs, width, x, y);
        flags.clear_visited(x, y);

        for ci in (runlen + 1)..lim {
            cleanup_sample_regular(
                decoder,
                band,
                bitplane,
                flags,
                magnitudes,
                signs,
                width,
                x,
                k + ci,
            );
        }
    } else {
        for ci in 0..lim {
            cleanup_sample_regular(
                decoder,
                band,
                bitplane,
                flags,
                magnitudes,
                signs,
                width,
                x,
                k + ci,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cleanup_sample_regular(
    decoder: &mut MqDecoder<'_>,
    band: BandOrientation,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    signs: &mut [u8],
    width: usize,
    x: usize,
    y: usize,
) {
    let visited = flags.is_visited(x, y);
    let significant = flags.is_significant(x, y);
    if significant || visited {
        flags.clear_visited(x, y);
        return;
    }

    let neighbour_mask = flags.neighbour_mask(x, y);
    let ctx = if neighbour_mask != 0 {
        zero_coding_context(band, neighbour_mask)
    } else {
        T1_CTXNO_ZC
    };
    let plane_bit = decoder.decode_with_ctx(ctx);
    flags.clear_visited(x, y);
    if plane_bit != 0 {
        decode_new_significant(decoder, bitplane, flags, magnitudes, signs, width, x, y);
    }
}

fn decode_new_significant(
    decoder: &mut MqDecoder<'_>,
    bitplane: u8,
    flags: &mut FlagGrid,
    magnitudes: &mut [u32],
    signs: &mut [u8],
    width: usize,
    x: usize,
    y: usize,
) {
    let (sign_lut_index, _) = flags.cardinal_sign_context(x, y);
    let sign_ctx = sign_context(sign_lut_index);
    let prediction = sign_prediction_bit(sign_lut_index);
    let sign_bit = decoder.decode_with_ctx(sign_ctx) ^ prediction;
    let idx = y * width + x;
    magnitudes[idx] |= 1u32 << bitplane;
    signs[idx] = sign_bit;
    flags.mark_significant(x, y, sign_bit);
}

fn clear_visited_all(flags: &mut FlagGrid, width: usize, height: usize) {
    for y in 0..height {
        for x in 0..width {
            flags.clear_visited(x, y);
        }
    }
}

fn copy_block_to_tile(
    tile_width: usize,
    tile: &mut [i32],
    block_x0: u32,
    block_y0: u32,
    block: &DecodedCodeBlockCoefficients,
) -> Result<()> {
    let block_x0 = usize::try_from(block_x0).map_err(|_| invalid("block x0 exceeds usize"))?;
    let block_y0 = usize::try_from(block_y0).map_err(|_| invalid("block y0 exceeds usize"))?;
    for y in 0..block.height {
        let dst = (block_y0 + y)
            .checked_mul(tile_width)
            .and_then(|row| row.checked_add(block_x0))
            .ok_or_else(|| invalid("block copy offset overflow"))?;
        let src = y * block.width;
        let dst_end = dst + block.width;
        let src_end = src + block.width;
        let dst_slice = tile
            .get_mut(dst..dst_end)
            .ok_or_else(|| invalid("decoded block extends past tile"))?;
        dst_slice.copy_from_slice(&block.coefficients[src..src_end]);
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> Jp2LamError {
    Jp2LamError::DecodeFailed(message.into())
}

#[cfg(test)]
mod tests {
    use super::decode_codeblock;
    use crate::mq::{MqCoder, T1_CTXNO_MAG, T1_CTXNO_ZC};
    use crate::plan::BandOrientation;
    use crate::tier1::helpers::{sign_context, sign_prediction_bit};

    #[test]
    fn cleanup_only_single_sample_decodes_positive_one() {
        let mut coder = MqCoder::new();
        coder.encode_with_ctx(T1_CTXNO_ZC, 1);
        let sign_ctx = sign_context(0);
        let prediction = sign_prediction_bit(0);
        coder.encode_with_ctx(sign_ctx, prediction);
        let bytes = coder.finish();

        let decoded =
            decode_codeblock(1, 1, BandOrientation::Ll, 1, 0, 1, &bytes).expect("decode");
        assert_eq!(decoded.coefficients, vec![1]);
    }

    #[test]
    fn cleanup_and_refinement_single_sample_decodes_negative_three() {
        let mut coder = MqCoder::new();
        coder.encode_with_ctx(T1_CTXNO_ZC, 1);
        let sign_ctx = sign_context(0);
        let prediction = sign_prediction_bit(0);
        coder.encode_with_ctx(sign_ctx, 1 ^ prediction);
        coder.encode_with_ctx(T1_CTXNO_MAG, 1);
        let bytes = coder.finish();

        let decoded =
            decode_codeblock(1, 1, BandOrientation::Ll, 2, 0, 3, &bytes).expect("decode");
        assert_eq!(decoded.coefficients, vec![-3]);
    }

    #[test]
    fn non_empty_codeblock_without_bytes_fails_fast() {
        let err = decode_codeblock(1, 1, BandOrientation::Ll, 1, 0, 1, &[])
            .expect_err("empty compressed bytes should fail")
            .to_string();

        assert!(err.contains("no compressed bytes"), "{err}");
    }

    #[test]
    fn impossible_pass_count_fails_fast() {
        let err = decode_codeblock(1, 1, BandOrientation::Ll, 1, 0, 2, &[0])
            .expect_err("too many passes for one bit-plane should fail")
            .to_string();

        assert!(err.contains("exceeds maximum"), "{err}");
    }
}
