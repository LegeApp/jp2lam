//! Annex A marker-segment decoding for the narrow decoder path.

use crate::error::{Jp2LamError, Result};
use crate::j2k::types::{CodestreamParts, TilePartHeader};
use crate::j2k::{
    MARKER_CAP, MARKER_COC, MARKER_COM, MARKER_CRG, MARKER_PLM, MARKER_PLT, MARKER_POC, MARKER_PPM,
    MARKER_PPT, MARKER_QCC, MARKER_RGN, MARKER_TLM,
};

use super::markers::{MARKER_COD, MARKER_QCD, MARKER_SIZ};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodestreamHeader {
    pub siz: SizSegment,
    pub cod: CodSegment,
    pub qcd: QcdSegment,
    pub comment_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizSegment {
    pub rsiz: u16,
    pub width: u32,
    pub height: u32,
    pub x_origin: u32,
    pub y_origin: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tile_x_origin: u32,
    pub tile_y_origin: u32,
    pub components: Vec<ComponentSiz>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentSiz {
    pub precision: u8,
    pub signed: bool,
    pub dx: u8,
    pub dy: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodSegment {
    pub progression_order: ProgressionOrder,
    pub layers: u16,
    pub use_mct: bool,
    pub decomposition_levels: u8,
    pub code_block_width: u32,
    pub code_block_height: u32,
    pub code_block_style: CodeBlockStyle,
    pub transform: WaveletTransform,
    pub uses_precincts: bool,
    pub sop_markers: bool,
    pub eph_markers: bool,
    pub precinct_sizes: Vec<PrecinctSize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrecinctSize {
    pub pp_x: u8,
    pub pp_y: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressionOrder {
    Lrcp,
    Rlcp,
    Rpcl,
    Pcrl,
    Cprl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveletTransform {
    Irreversible97,
    Reversible53,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeBlockStyle {
    pub bypass: bool,
    pub reset_contexts: bool,
    pub terminate_each_pass: bool,
    pub vertical_causal: bool,
    pub predictable_termination: bool,
    pub segmentation_symbols: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QcdSegment {
    pub style: QuantizationStyle,
    pub guard_bits: u8,
    pub steps: Vec<QuantizationStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizationStyle {
    NoQuantization,
    ScalarDerived,
    ScalarExpounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantizationStep {
    pub exponent: u8,
    pub mantissa: u16,
}

impl CodestreamHeader {
    #[allow(dead_code)]
    pub(crate) fn from_parts(parts: &CodestreamParts) -> Result<Self> {
        let first_tile = parts
            .tile_parts
            .first()
            .ok_or_else(|| invalid("codestream has no tile-parts"))?;
        Self::from_marker_segments(
            parts.main_header_segments.iter().map(Vec::as_slice),
            first_tile.header,
            parts.tile_parts.len(),
        )
    }

    pub(crate) fn from_marker_segments<'a, I>(
        main_header_segments: I,
        first_tile_header: TilePartHeader,
        tile_part_count: usize,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = &'a [u8]>,
    {
        Self::from_marker_segments_with_tile_headers(
            main_header_segments,
            std::iter::empty::<&'a [u8]>(),
            first_tile_header,
            tile_part_count,
        )
    }

    pub(crate) fn from_marker_segments_with_tile_headers<'a, I, J>(
        main_header_segments: I,
        tile_header_segments: J,
        first_tile_header: TilePartHeader,
        tile_part_count: usize,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = &'a [u8]>,
        J: IntoIterator<Item = &'a [u8]>,
    {
        if tile_part_count != 1 {
            return Err(invalid(
                "unsupported tiling: decoder currently supports one full-image tile in one tile-part",
            ));
        }
        let mut siz = None;
        let mut cod = None;
        let mut qcd = None;
        let mut comment_count = 0;

        for segment in main_header_segments {
            let marker = marker(segment)?;
            reject_unsupported_main_marker(marker)?;
            match marker {
                MARKER_SIZ => siz = Some(parse_siz(segment)?),
                MARKER_COD => cod = Some(parse_cod(segment)?),
                MARKER_QCD => qcd = Some(parse_qcd(segment)?),
                MARKER_COM => comment_count += 1,
                _ => unreachable!("main marker allowlist accepted an unhandled marker"),
            }
        }
        for segment in tile_header_segments {
            let marker = marker(segment)?;
            reject_unsupported_tile_marker(marker)?;
            if marker == MARKER_COM {
                comment_count += 1;
            }
        }

        let siz = siz.ok_or_else(|| invalid("codestream main header lacks SIZ"))?;
        let cod = cod.ok_or_else(|| invalid("codestream main header lacks COD"))?;
        let qcd = qcd.ok_or_else(|| invalid("codestream main header lacks QCD"))?;
        validate_decoder_scope(first_tile_header, &siz, &cod, &qcd)?;
        Ok(Self {
            siz,
            cod,
            qcd,
            comment_count,
        })
    }
}

fn parse_siz(segment: &[u8]) -> Result<SizSegment> {
    let body = body(segment)?;
    if body.len() < 38 {
        return Err(invalid("SIZ marker body is too short"));
    }
    let component_count = read_u16(body, 34)? as usize;
    let expected_len = 36usize
        .checked_add(
            component_count
                .checked_mul(3)
                .ok_or_else(|| invalid("SIZ component length overflow"))?,
        )
        .ok_or_else(|| invalid("SIZ length overflow"))?;
    if body.len() != expected_len {
        return Err(invalid("SIZ marker length does not match component count"));
    }

    let mut components = Vec::with_capacity(component_count);
    for idx in 0..component_count {
        let off = 36 + idx * 3;
        let ssiz = body[off];
        components.push(ComponentSiz {
            precision: (ssiz & 0x7f) + 1,
            signed: ssiz & 0x80 != 0,
            dx: body[off + 1],
            dy: body[off + 2],
        });
    }

    Ok(SizSegment {
        rsiz: read_u16(body, 0)?,
        width: read_u32(body, 2)?,
        height: read_u32(body, 6)?,
        x_origin: read_u32(body, 10)?,
        y_origin: read_u32(body, 14)?,
        tile_width: read_u32(body, 18)?,
        tile_height: read_u32(body, 22)?,
        tile_x_origin: read_u32(body, 26)?,
        tile_y_origin: read_u32(body, 30)?,
        components,
    })
}

fn parse_cod(segment: &[u8]) -> Result<CodSegment> {
    let body = body(segment)?;
    if body.len() < 10 {
        return Err(invalid("COD marker body is too short"));
    }
    let scod = body[0];
    let uses_precincts = scod & 0x01 != 0;
    let progression_order = match body[1] {
        0 => ProgressionOrder::Lrcp,
        1 => ProgressionOrder::Rlcp,
        2 => ProgressionOrder::Rpcl,
        3 => ProgressionOrder::Pcrl,
        4 => ProgressionOrder::Cprl,
        value => return Err(invalid(format!("unsupported progression order {value}"))),
    };
    let decomposition_levels = body[5];
    let expected_len = if uses_precincts {
        10usize
            .checked_add(usize::from(decomposition_levels) + 1)
            .ok_or_else(|| invalid("COD precinct length overflow"))?
    } else {
        10
    };
    if body.len() != expected_len {
        return Err(invalid("COD marker length does not match Scod precinct flag"));
    }

    let style = body[8];
    if style & !0x3f != 0 {
        return Err(invalid("COD code-block style has reserved bits set"));
    }
    let transform = match body[9] {
        0 => WaveletTransform::Irreversible97,
        1 => WaveletTransform::Reversible53,
        value => return Err(invalid(format!("unsupported wavelet transform {value}"))),
    };
    let precinct_sizes = body[10..]
        .iter()
        .map(|value| PrecinctSize {
            pp_x: value & 0x0f,
            pp_y: value >> 4,
        })
        .collect();

    let code_block_width = code_block_dimension(body[6], "width")?;
    let code_block_height = code_block_dimension(body[7], "height")?;

    Ok(CodSegment {
        progression_order,
        layers: read_u16(body, 2)?,
        use_mct: body[4] != 0,
        decomposition_levels,
        code_block_width,
        code_block_height,
        code_block_style: CodeBlockStyle {
            bypass: style & 0x01 != 0,
            reset_contexts: style & 0x02 != 0,
            terminate_each_pass: style & 0x04 != 0,
            vertical_causal: style & 0x08 != 0,
            predictable_termination: style & 0x10 != 0,
            segmentation_symbols: style & 0x20 != 0,
        },
        transform,
        uses_precincts,
        sop_markers: scod & 0x02 != 0,
        eph_markers: scod & 0x04 != 0,
        precinct_sizes,
    })
}

fn parse_qcd(segment: &[u8]) -> Result<QcdSegment> {
    let body = body(segment)?;
    if body.is_empty() {
        return Err(invalid("QCD marker body is too short"));
    }
    let sqcd = body[0];
    let guard_bits = sqcd >> 5;
    let style = match sqcd & 0x1f {
        0 => QuantizationStyle::NoQuantization,
        1 => QuantizationStyle::ScalarDerived,
        2 => QuantizationStyle::ScalarExpounded,
        value => return Err(invalid(format!("unsupported QCD style {value}"))),
    };
    let payload = &body[1..];
    let steps = match style {
        QuantizationStyle::NoQuantization => payload
            .iter()
            .map(|value| QuantizationStep {
                exponent: value >> 3,
                mantissa: 0,
            })
            .collect(),
        QuantizationStyle::ScalarDerived | QuantizationStyle::ScalarExpounded => {
            if payload.len() % 2 != 0 {
                return Err(invalid("QCD 16-bit step payload has odd length"));
            }
            let mut steps = Vec::with_capacity(payload.len() / 2);
            for chunk in payload.chunks_exact(2) {
                let packed = u16::from_be_bytes([chunk[0], chunk[1]]);
                steps.push(QuantizationStep {
                    exponent: (packed >> 11) as u8,
                    mantissa: packed & 0x07ff,
                });
            }
            steps
        }
    };
    Ok(QcdSegment {
        style,
        guard_bits,
        steps,
    })
}

fn validate_decoder_scope(
    first_tile_header: TilePartHeader,
    siz: &SizSegment,
    cod: &CodSegment,
    qcd: &QcdSegment,
) -> Result<()> {
    if first_tile_header.tile_index != 0 {
        return Err(invalid("only tile index 0 is supported"));
    }
    if first_tile_header.part_index != 0 {
        return Err(invalid("only first tile-part is supported"));
    }
    if first_tile_header.total_parts > 1 {
        return Err(invalid(
            "unsupported tiling: multiple tile-parts are not implemented",
        ));
    }
    if siz.rsiz != 0 {
        return Err(invalid("only Part 1 Rsiz=0 codestreams are supported"));
    }
    if siz.x_origin != 0 || siz.y_origin != 0 || siz.tile_x_origin != 0 || siz.tile_y_origin != 0 {
        return Err(invalid("non-zero image or tile origins are unsupported"));
    }
    if siz.tile_width != siz.width || siz.tile_height != siz.height {
        return Err(invalid(
            "unsupported tiling: decoder currently supports only a single full-image tile",
        ));
    }
    if siz.components.len() != 1 && siz.components.len() != 3 {
        return Err(invalid(format!(
            "unsupported component layout: decoder currently supports one grayscale component or three sRGB components, found {} components",
            siz.components.len()
        )));
    }
    for (idx, component) in siz.components.iter().enumerate() {
        if component.precision != 8 || component.signed {
            return Err(invalid(format!(
                "unsupported sample precision: decoder currently supports unsigned 8-bit samples, component {idx} is {}-bit {}",
                component.precision,
                if component.signed { "signed" } else { "unsigned" }
            )));
        }
        if component.dx != 1 || component.dy != 1 {
            return Err(invalid(format!(
                "subsampled components are unsupported: component {idx} has dx={} dy={}",
                component.dx, component.dy
            )));
        }
    }
    if cod.progression_order != ProgressionOrder::Lrcp {
        return Err(invalid("only LRCP progression is supported"));
    }
    let max_decompositions = max_dwt_decompositions(siz.width, siz.height);
    if cod.decomposition_levels > max_decompositions {
        return Err(invalid(format!(
            "COD decomposition levels {} exceed DWT limit {} for image dimensions {}x{}",
            cod.decomposition_levels,
            max_decompositions,
            siz.width,
            siz.height
        )));
    }
    if cod.layers == 0 {
        return Err(invalid("COD must signal at least one layer"));
    }
    if cod.layers != 1 {
        return Err(invalid(format!(
            "unsupported quality layers: decoder currently supports exactly one layer, found {}",
            cod.layers
        )));
    }
    if siz.components.len() == 1 && cod.use_mct {
        return Err(invalid("MCT is unsupported for grayscale decoder slice"));
    }
    if siz.components.len() == 3 && !cod.use_mct {
        return Err(invalid(
            "unsupported RGB codestream: expected MCT=1 for Archive.org sRGB profile",
        ));
    }
    if cod.uses_precincts || cod.sop_markers || cod.eph_markers {
        return Err(invalid(
            "unsupported packet syntax: precinct partitioning, SOP markers, and EPH markers are not implemented",
        ));
    }
    if cod.code_block_style.bypass {
        return Err(invalid("unsupported code-block style: arithmetic bypass is not implemented"));
    }
    if cod.code_block_style.reset_contexts {
        return Err(invalid("unsupported code-block style: context reset is not implemented"));
    }
    if cod.code_block_style.terminate_each_pass {
        return Err(invalid(
            "unsupported code-block style: pass termination is not implemented",
        ));
    }
    if cod.code_block_style.vertical_causal {
        return Err(invalid(
            "unsupported code-block style: vertical causal context is not implemented",
        ));
    }
    if cod.code_block_style.predictable_termination {
        return Err(invalid(
            "unsupported code-block style: predictable termination is not implemented",
        ));
    }
    if cod.code_block_style.segmentation_symbols {
        return Err(invalid(
            "unsupported code-block style: segmentation symbols are not implemented",
        ));
    }
    match (cod.transform, qcd.style) {
        (WaveletTransform::Reversible53, QuantizationStyle::NoQuantization) => {}
        (WaveletTransform::Irreversible97, QuantizationStyle::ScalarExpounded) => {}
        (WaveletTransform::Irreversible97, QuantizationStyle::ScalarDerived) => {
            return Err(invalid(
                "unsupported quantization: scalar-derived QCD is not implemented",
            ));
        }
        (WaveletTransform::Irreversible97, QuantizationStyle::NoQuantization) => {
            return Err(invalid(
                "unsupported quantization: irreversible 9/7 requires scalar quantization",
            ));
        }
        (WaveletTransform::Reversible53, QuantizationStyle::ScalarDerived | QuantizationStyle::ScalarExpounded) => {
            return Err(invalid(
                "unsupported quantization: reversible 5/3 currently supports no-quantization QCD only",
            ));
        }
    }
    if cod.code_block_width > 1024
        || cod.code_block_height > 1024
        || cod.code_block_width * cod.code_block_height > 4096
    {
        return Err(invalid("COD code-block size exceeds Part 1 limits"));
    }
    if qcd.steps.is_empty() {
        return Err(invalid("QCD must provide at least one quantization step"));
    }
    let expected_qcd_steps = 1usize + usize::from(cod.decomposition_levels) * 3;
    match qcd.style {
        QuantizationStyle::NoQuantization | QuantizationStyle::ScalarExpounded => {
            if qcd.steps.len() != expected_qcd_steps {
                return Err(invalid(format!(
                    "QCD step count {} does not match decomposition levels {}; expected {} steps",
                    qcd.steps.len(),
                    cod.decomposition_levels,
                    expected_qcd_steps
                )));
            }
        }
        QuantizationStyle::ScalarDerived => {}
    }
    Ok(())
}

fn code_block_dimension(encoded: u8, axis: &str) -> Result<u32> {
    if encoded > 8 {
        return Err(invalid(format!(
            "unsupported code-block {axis}: encoded exponent {encoded} exceeds Part 1 limit"
        )));
    }
    Ok(1u32 << (u32::from(encoded) + 2))
}

fn max_dwt_decompositions(width: u32, height: u32) -> u8 {
    let min_dim = width.min(height);
    if min_dim <= 1 {
        return 0;
    }
    ((u32::BITS - 1) - min_dim.leading_zeros()) as u8
}

fn reject_unsupported_main_marker(marker: u16) -> Result<()> {
    match marker {
        MARKER_SIZ | MARKER_COD | MARKER_QCD | MARKER_COM => Ok(()),
        MARKER_CAP => unsupported_marker(marker, "CAP capabilities marker"),
        MARKER_COC => unsupported_marker(marker, "COC component coding-style override"),
        MARKER_TLM => unsupported_marker(marker, "TLM tile-part length marker"),
        MARKER_PLM => unsupported_marker(marker, "PLM packet-length marker"),
        MARKER_PLT => unsupported_marker(marker, "PLT packet-length marker"),
        MARKER_QCC => unsupported_marker(marker, "QCC component quantization override"),
        MARKER_RGN => unsupported_marker(marker, "RGN region-of-interest marker"),
        MARKER_POC => unsupported_marker(marker, "POC progression-order change marker"),
        MARKER_PPM => unsupported_marker(marker, "PPM packed packet headers"),
        MARKER_PPT => unsupported_marker(marker, "PPT packed packet headers"),
        MARKER_CRG => unsupported_marker(marker, "CRG component registration marker"),
        _ => Err(unsupported(format!(
            "unsupported codestream marker 0x{marker:04x} in main header"
        ))),
    }
}

fn reject_unsupported_tile_marker(marker: u16) -> Result<()> {
    match marker {
        MARKER_COM => Ok(()),
        MARKER_COD => unsupported_marker(marker, "tile-part COD coding-style override"),
        MARKER_COC => unsupported_marker(marker, "tile-part COC component coding-style override"),
        MARKER_QCD => unsupported_marker(marker, "tile-part QCD quantization override"),
        MARKER_QCC => unsupported_marker(marker, "tile-part QCC component quantization override"),
        MARKER_RGN => unsupported_marker(marker, "tile-part RGN region-of-interest marker"),
        MARKER_POC => unsupported_marker(marker, "tile-part POC progression-order change marker"),
        MARKER_PLT => unsupported_marker(marker, "tile-part PLT packet-length marker"),
        MARKER_PPT => unsupported_marker(marker, "tile-part PPT packed packet headers"),
        _ => Err(unsupported(format!(
            "unsupported codestream marker 0x{marker:04x} in tile-part header"
        ))),
    }
}

fn unsupported_marker(marker: u16, name: &str) -> Result<()> {
    Err(unsupported(format!(
        "unsupported JPEG 2000 feature: {name} (marker 0x{marker:04x})"
    )))
}

fn marker(segment: &[u8]) -> Result<u16> {
    if segment.len() < 4 {
        return Err(invalid("truncated marker segment"));
    }
    Ok(u16::from_be_bytes([segment[0], segment[1]]))
}

fn body(segment: &[u8]) -> Result<&[u8]> {
    if segment.len() < 4 {
        return Err(invalid("truncated marker segment"));
    }
    let marker_len = u16::from_be_bytes([segment[2], segment[3]]) as usize;
    if marker_len < 2 || marker_len + 2 != segment.len() {
        return Err(invalid("marker segment length mismatch"));
    }
    Ok(&segment[4..])
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| invalid("unexpected end of marker u16"))?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("unexpected end of marker u32"))?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn invalid(message: impl Into<String>) -> Jp2LamError {
    Jp2LamError::DecodeFailed(message.into())
}

fn unsupported(message: impl Into<String>) -> Jp2LamError {
    Jp2LamError::UnsupportedFeature(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::j2k::{
        MARKER_CAP, MARKER_COC, MARKER_CRG, MARKER_PLM, MARKER_PLT, MARKER_POC, MARKER_PPM,
        MARKER_PPT, MARKER_QCC, MARKER_RGN, MARKER_TLM,
    };

    #[test]
    fn unsupported_main_marker_fails_fast_with_feature_name() {
        let mut segments = supported_segments();
        segments.push(segment(MARKER_COC, &[0x00, 0x00]));

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("COC should be rejected");

        assert!(matches!(err, Jp2LamError::UnsupportedFeature(_)), "{err:?}");
        let err = err.to_string();
        assert!(err.contains("COC component coding-style override"), "{err}");
    }

    #[test]
    fn unsupported_main_marker_matrix_fails_fast_with_feature_names() {
        for (marker, expected) in [
            (MARKER_CAP, "CAP capabilities marker"),
            (MARKER_COC, "COC component coding-style override"),
            (MARKER_TLM, "TLM tile-part length marker"),
            (MARKER_PLM, "PLM packet-length marker"),
            (MARKER_PLT, "PLT packet-length marker"),
            (MARKER_QCC, "QCC component quantization override"),
            (MARKER_RGN, "RGN region-of-interest marker"),
            (MARKER_POC, "POC progression-order change marker"),
            (MARKER_PPM, "PPM packed packet headers"),
            (MARKER_PPT, "PPT packed packet headers"),
            (MARKER_CRG, "CRG component registration marker"),
        ] {
            let mut segments = supported_segments();
            segments.push(segment(marker, &[0x00, 0x00]));

            let err = CodestreamHeader::from_marker_segments(
                segments.iter().map(Vec::as_slice),
                tile_header(0),
                1,
            )
            .unwrap_err()
            .to_string();

            assert!(err.contains(expected), "marker 0x{marker:04x}: {err}");
        }
    }

    #[test]
    fn unsupported_tile_header_marker_fails_fast_with_feature_name() {
        let tile_segments = [segment(MARKER_QCC, &[0x00, 0x00])];

        let err = CodestreamHeader::from_marker_segments_with_tile_headers(
            supported_segments().iter().map(Vec::as_slice),
            tile_segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("QCC should be rejected")
        .to_string();

        assert!(err.contains("tile-part QCC component quantization override"), "{err}");
    }

    #[test]
    fn unsupported_tile_header_marker_matrix_fails_fast_with_feature_names() {
        for (marker, expected) in [
            (MARKER_COD, "tile-part COD coding-style override"),
            (MARKER_COC, "tile-part COC component coding-style override"),
            (MARKER_QCD, "tile-part QCD quantization override"),
            (MARKER_QCC, "tile-part QCC component quantization override"),
            (MARKER_RGN, "tile-part RGN region-of-interest marker"),
            (MARKER_POC, "tile-part POC progression-order change marker"),
            (MARKER_PLT, "tile-part PLT packet-length marker"),
            (MARKER_PPT, "tile-part PPT packed packet headers"),
        ] {
            let tile_segments = [segment(marker, &[0x00, 0x00])];

            let err = CodestreamHeader::from_marker_segments_with_tile_headers(
                supported_segments().iter().map(Vec::as_slice),
                tile_segments.iter().map(Vec::as_slice),
                tile_header(0),
                1,
            )
            .unwrap_err()
            .to_string();

            assert!(err.contains(expected), "marker 0x{marker:04x}: {err}");
        }
    }

    #[test]
    fn multiple_layers_fail_before_packet_decode() {
        let mut segments = supported_segments();
        segments[1] = cod_segment(2, 0);

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("multiple layers should be rejected")
        .to_string();

        assert!(err.contains("unsupported quality layers"), "{err}");
    }

    #[test]
    fn hdr_precision_fails_before_packet_decode() {
        let mut segments = supported_segments();
        segments[0] = siz_segment(12, false, 1);

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("HDR precision should be rejected")
        .to_string();

        assert!(err.contains("unsupported sample precision"), "{err}");
    }

    #[test]
    fn progression_change_marker_fails_fast() {
        let mut segments = supported_segments();
        segments.push(segment(MARKER_POC, &[0x00, 0x00]));

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("POC should be rejected")
        .to_string();

        assert!(err.contains("POC progression-order change marker"), "{err}");
    }

    #[test]
    fn scalar_derived_quantization_fails_before_packet_decode() {
        let mut segments = supported_segments();
        segments[2] = qcd_segment_with_style(0x21);

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("scalar-derived QCD should be rejected")
        .to_string();

        assert!(err.contains("scalar-derived QCD"), "{err}");
    }

    #[test]
    fn oversized_codeblock_dimension_fails_during_cod_parse() {
        let mut segments = supported_segments();
        segments[1][10] = 9;

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("oversized code-block width should be rejected")
        .to_string();

        assert!(err.contains("code-block width"), "{err}");
    }

    #[test]
    fn unsupported_codeblock_style_matrix_fails_fast() {
        for (style, expected) in [
            (0x01, "arithmetic bypass"),
            (0x02, "context reset"),
            (0x04, "pass termination"),
            (0x08, "vertical causal"),
            (0x10, "predictable termination"),
            (0x20, "segmentation symbols"),
        ] {
            let mut segments = supported_segments();
            segments[1] = cod_segment(1, style);

            let err = CodestreamHeader::from_marker_segments(
                segments.iter().map(Vec::as_slice),
                tile_header(0),
                1,
            )
            .unwrap_err()
            .to_string();

            assert!(err.contains(expected), "style 0x{style:02x}: {err}");
        }
    }

    #[test]
    fn short_qcd_step_count_fails_before_packet_decode() {
        let mut segments = supported_segments();
        segments[2] = qcd_segment_with_step_count(0x22, 15);

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("short QCD step table should fail")
        .to_string();

        assert!(err.contains("QCD step count"), "{err}");
    }

    #[test]
    fn extra_qcd_step_count_fails_before_packet_decode() {
        let mut segments = supported_segments();
        segments[2] = qcd_segment_with_step_count(0x22, 17);

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("extra QCD step table should fail")
        .to_string();

        assert!(err.contains("QCD step count"), "{err}");
    }

    #[test]
    fn excessive_decomposition_levels_fail_before_packet_decode() {
        let mut segments = supported_segments();
        segments[0] = siz_segment_with_size(8, false, 1, 8, 8);

        let err = CodestreamHeader::from_marker_segments(
            segments.iter().map(Vec::as_slice),
            tile_header(0),
            1,
        )
        .expect_err("excessive decomposition levels should fail")
        .to_string();

        assert!(err.contains("decomposition levels"), "{err}");
    }

    fn supported_segments() -> Vec<Vec<u8>> {
        vec![siz_segment(8, false, 1), cod_segment(1, 0), qcd_segment()]
    }

    fn tile_header(total_parts: u8) -> TilePartHeader {
        TilePartHeader {
            tile_index: 0,
            part_index: 0,
            total_parts,
        }
    }

    fn siz_segment(precision: u8, signed: bool, components: u16) -> Vec<u8> {
        siz_segment_with_size(precision, signed, components, 256, 256)
    }

    fn siz_segment_with_size(
        precision: u8,
        signed: bool,
        components: u16,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&width.to_be_bytes());
        body.extend_from_slice(&height.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&width.to_be_bytes());
        body.extend_from_slice(&height.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&components.to_be_bytes());
        for _ in 0..components {
            body.push((precision - 1) | if signed { 0x80 } else { 0 });
            body.push(1);
            body.push(1);
        }
        segment(MARKER_SIZ, &body)
    }

    fn cod_segment(layers: u16, style: u8) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(0);
        body.push(0);
        body.extend_from_slice(&layers.to_be_bytes());
        body.push(0);
        body.push(5);
        body.push(4);
        body.push(4);
        body.push(style);
        body.push(0);
        segment(MARKER_COD, &body)
    }

    fn qcd_segment() -> Vec<u8> {
        qcd_segment_with_step_count(0x22, 16)
    }

    fn qcd_segment_with_style(sqcd: u8) -> Vec<u8> {
        qcd_segment_with_step_count(sqcd, 16)
    }

    fn qcd_segment_with_step_count(sqcd: u8, step_count: usize) -> Vec<u8> {
        let exponent = 8u16 << 11;
        let mut body = vec![sqcd];
        for _ in 0..step_count {
            body.extend_from_slice(&exponent.to_be_bytes());
        }
        segment(MARKER_QCD, &body)
    }

    fn segment(marker: u16, body: &[u8]) -> Vec<u8> {
        let marker_len = u16::try_from(body.len() + 2).expect("marker length");
        let mut out = Vec::with_capacity(body.len() + 4);
        out.extend_from_slice(&marker.to_be_bytes());
        out.extend_from_slice(&marker_len.to_be_bytes());
        out.extend_from_slice(body);
        out
    }
}
