use crate::error::{Jp2LamError, Result};
use crate::plan::{EncodingPlan, QuantizationStyle};

use super::markers::{encode_cod, encode_qcd, encode_siz, MARKER_COD, MARKER_QCD, MARKER_SIZ};
use super::types::{CodestreamParts, TilePart};
use super::{MARKER_EOC, MARKER_SOC, MARKER_SOD, MARKER_SOT};

impl CodestreamParts {
    pub(crate) fn encode(&self, plan: &EncodingPlan) -> Result<Vec<u8>> {
        if plan.component_count == 0 {
            return Err(Jp2LamError::EncodeFailed(
                "cannot emit codestream with zero components".to_string(),
            ));
        }

        let capacity = 2
            + self
                .main_header_segments
                .iter()
                .map(Vec::len)
                .sum::<usize>()
            + self
                .tile_parts
                .iter()
                .map(|tile_part| {
                    12 + tile_part
                        .header_segments
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>()
                        + 2
                        + tile_part.payload.byte_len()
                })
                .sum::<usize>()
            + 2;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&MARKER_SOC.to_be_bytes());
        for segment in &self.main_header_segments {
            out.extend_from_slice(&rewrite_main_header_segment(segment, plan)?);
        }
        for tile_part in &self.tile_parts {
            out.extend_from_slice(&encode_sot_segment(tile_part)?);
            for header in &tile_part.header_segments {
                out.extend_from_slice(header);
            }
            out.extend_from_slice(&MARKER_SOD.to_be_bytes());
            tile_part.payload.write_to(&mut out);
        }
        out.extend_from_slice(&MARKER_EOC.to_be_bytes());
        Ok(out)
    }
}

fn encode_sot_segment(tile_part: &TilePart) -> Result<[u8; 12]> {
    let psot = 12usize
        .checked_add(
            tile_part
                .header_segments
                .iter()
                .map(Vec::len)
                .sum::<usize>(),
        )
        .and_then(|len| len.checked_add(2))
        .and_then(|len| len.checked_add(tile_part.payload.byte_len()))
        .ok_or_else(|| Jp2LamError::EncodeFailed("tile-part length overflow".to_string()))?;
    let psot = u32::try_from(psot).map_err(|_| {
        Jp2LamError::EncodeFailed("tile-part length exceeds Psot range".to_string())
    })?;

    let mut segment = [0u8; 12];
    segment[0..2].copy_from_slice(&MARKER_SOT.to_be_bytes());
    segment[2..4].copy_from_slice(&10u16.to_be_bytes());
    segment[4..6].copy_from_slice(&tile_part.header.tile_index.to_be_bytes());
    segment[6..10].copy_from_slice(&psot.to_be_bytes());
    segment[10] = tile_part.header.part_index;
    segment[11] = tile_part.header.total_parts;
    Ok(segment)
}

fn rewrite_main_header_segment(segment: &[u8], plan: &EncodingPlan) -> Result<Vec<u8>> {
    let Some(marker) = segment_marker(segment) else {
        return Ok(segment.to_vec());
    };

    match marker {
        MARKER_SIZ => {
            let local = encode_siz(plan)?;
            if local.len() == segment.len() {
                Ok(local)
            } else {
                Ok(segment.to_vec())
            }
        }
        MARKER_COD => {
            let local = encode_cod(plan);
            if local.len() == segment.len() {
                Ok(local)
            } else {
                Ok(segment.to_vec())
            }
        }
        MARKER_QCD => match encode_qcd(plan) {
            Ok(local)
                if matches!(plan.quantization_style, QuantizationStyle::NoQuantization)
                    && local.len() == segment.len() =>
            {
                Ok(local)
            }
            Ok(_) | Err(_) => Ok(segment.to_vec()),
        },
        _ => Ok(segment.to_vec()),
    }
}

fn segment_marker(segment: &[u8]) -> Option<u16> {
    let bytes = segment.get(0..2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}
