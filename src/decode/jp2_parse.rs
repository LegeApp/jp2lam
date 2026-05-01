//! JP2 box parsing for the decoder.
//!
//! Implements the small Annex I subset needed by the current decoder plan:
//! signature, file type, JP2 header with `ihdr` and enumerated `colr`, and the
//! first contiguous codestream box (`jp2c`, I.5.4).

use crate::error::{Jp2LamError, Result};
use crate::model::ColorSpace;

const BOX_SIGNATURE: [u8; 4] = *b"jP  ";
const BOX_FILE_TYPE: [u8; 4] = *b"ftyp";
const BOX_JP2_HEADER: [u8; 4] = *b"jp2h";
const BOX_IMAGE_HEADER: [u8; 4] = *b"ihdr";
const BOX_BITS_PER_COMPONENT: [u8; 4] = *b"bpcc";
const BOX_COLOR_SPEC: [u8; 4] = *b"colr";
const BOX_PALETTE: [u8; 4] = *b"pclr";
const BOX_COMPONENT_MAPPING: [u8; 4] = *b"cmap";
const BOX_CHANNEL_DEFINITION: [u8; 4] = *b"cdef";
const BOX_CODESTREAM: [u8; 4] = *b"jp2c";

const JP2_SIGNATURE_PAYLOAD: [u8; 4] = [0x0d, 0x0a, 0x87, 0x0a];
const JP2_COMPRESSION_TYPE_J2K: u8 = 7;
const ENUM_SRGB: u32 = 16;
const ENUM_GRAY: u32 = 17;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedJp2<'a> {
    pub(crate) header: Jp2Header,
    pub(crate) codestream: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Jp2Header {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) component_count: u16,
    pub(crate) bits_per_component: u8,
    pub(crate) colorspace: ColorSpace,
    pub(crate) has_ipr_metadata: bool,
}

pub(crate) fn parse_jp2(bytes: &[u8]) -> Result<ParsedJp2<'_>> {
    let mut cursor = BoxCursor::new(bytes);
    let signature = cursor
        .next_box()?
        .ok_or_else(|| invalid("missing JP2 signature box"))?;
    if signature.box_type != BOX_SIGNATURE || signature.payload != JP2_SIGNATURE_PAYLOAD {
        return Err(invalid("invalid JP2 signature box"));
    }

    let file_type = cursor
        .next_box()?
        .ok_or_else(|| invalid("missing JP2 file type box"))?;
    if file_type.box_type != BOX_FILE_TYPE {
        return Err(invalid("JP2 file type box must follow signature box"));
    }
    validate_file_type(file_type.payload)?;

    let mut header = None;
    let mut codestream = None;
    while let Some(box_) = cursor.next_box()? {
        match box_.box_type {
            BOX_JP2_HEADER => header = Some(parse_jp2_header(box_.payload)?),
            BOX_CODESTREAM if codestream.is_none() => codestream = Some(box_.payload),
            BOX_CODESTREAM => {
                return Err(invalid(
                    "unsupported JP2 layout: multiple contiguous codestream boxes",
                ));
            }
            _ => {}
        }
    }

    let header = header.ok_or_else(|| invalid("missing JP2 header box"))?;
    let codestream = codestream.ok_or_else(|| invalid("missing contiguous codestream box"))?;
    Ok(ParsedJp2 { header, codestream })
}

fn validate_file_type(payload: &[u8]) -> Result<()> {
    if payload.len() < 12 {
        return Err(invalid("JP2 file type box is too short"));
    }
    if &payload[0..4] != b"jp2 " {
        return Err(invalid("JP2 brand is not jp2"));
    }
    if payload[8..].chunks_exact(4).any(|compat| compat == b"jp2 ") {
        Ok(())
    } else {
        Err(invalid("JP2 file type lacks jp2 compatibility brand"))
    }
}

fn parse_jp2_header(payload: &[u8]) -> Result<Jp2Header> {
    let mut cursor = BoxCursor::new(payload);
    let mut image_header = None;
    let mut colorspace = None;
    while let Some(box_) = cursor.next_box()? {
        match box_.box_type {
            BOX_IMAGE_HEADER => image_header = Some(parse_image_header(box_.payload)?),
            BOX_BITS_PER_COMPONENT => {
                return Err(unsupported(
                    "unsupported JP2 feature: per-component bit-depth box (bpcc)",
                ));
            }
            BOX_COLOR_SPEC if colorspace.is_none() => {
                colorspace = Some(parse_color_spec(box_.payload)?)
            }
            BOX_PALETTE => {
                return Err(unsupported("unsupported JP2 feature: palette box (pclr)"));
            }
            BOX_COMPONENT_MAPPING => {
                return Err(unsupported(
                    "unsupported JP2 feature: component mapping box (cmap)",
                ));
            }
            BOX_CHANNEL_DEFINITION => {
                return Err(unsupported(
                    "unsupported JP2 feature: channel definition box (cdef)",
                ));
            }
            _ => {}
        }
    }

    let mut header = image_header.ok_or_else(|| invalid("JP2 header lacks ihdr box"))?;
    header.colorspace = colorspace.ok_or_else(|| invalid("JP2 header lacks colr box"))?;
    Ok(header)
}

fn parse_image_header(payload: &[u8]) -> Result<Jp2Header> {
    if payload.len() != 14 {
        return Err(invalid("JP2 ihdr box must be 14 bytes"));
    }
    let height = read_u32(payload, 0)?;
    let width = read_u32(payload, 4)?;
    let component_count = read_u16(payload, 8)?;
    let bpc = payload[10];
    let compression_type = payload[11];
    if compression_type != JP2_COMPRESSION_TYPE_J2K {
        return Err(invalid("JP2 ihdr compression type is not JPEG 2000"));
    }
    if bpc == 0xff {
        return Err(unsupported(
            "unsupported JP2 feature: per-component bit depths require bpcc",
        ));
    }
    Ok(Jp2Header {
        width,
        height,
        component_count,
        bits_per_component: (bpc & 0x7f) + 1,
        colorspace: ColorSpace::Gray,
        has_ipr_metadata: payload[13] != 0,
    })
}

fn parse_color_spec(payload: &[u8]) -> Result<ColorSpace> {
    if payload.len() < 3 {
        return Err(invalid("JP2 colr box is too short"));
    }
    let method = payload[0];
    if method != 1 {
        return Err(unsupported("only enumerated JP2 colorspaces are supported"));
    }
    if payload.len() != 7 {
        return Err(invalid("enumerated JP2 colr box must be 7 bytes"));
    }
    match read_u32(payload, 3)? {
        ENUM_GRAY => Ok(ColorSpace::Gray),
        ENUM_SRGB => Ok(ColorSpace::Srgb),
        value => Err(unsupported(format!("unsupported JP2 EnumCS value {value}"))),
    }
}

#[derive(Debug, Clone, Copy)]
struct BoxRecord<'a> {
    box_type: [u8; 4],
    payload: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
struct BoxCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BoxCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn next_box(&mut self) -> Result<Option<BoxRecord<'a>>> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        if self.bytes.len().saturating_sub(self.pos) < 8 {
            return Err(invalid("truncated JP2 box header"));
        }

        let start = self.pos;
        let lbox = read_u32(self.bytes, start)? as u64;
        let box_type = read_box_type(self.bytes, start + 4)?;
        self.pos += 8;

        let end = match lbox {
            0 => self.bytes.len(),
            1 => {
                let xlbox = read_u64(self.bytes, self.pos)?;
                self.pos += 8;
                let end = start
                    .checked_add(
                        usize::try_from(xlbox)
                            .map_err(|_| invalid("JP2 XLBox exceeds usize"))?,
                    )
                    .ok_or_else(|| invalid("JP2 XLBox overflow"))?;
                if xlbox < 16 {
                    return Err(invalid("invalid extended JP2 box length"));
                }
                end
            }
            2..=7 => return Err(invalid("invalid JP2 box length below header size")),
            len => start
                .checked_add(
                    usize::try_from(len)
                        .map_err(|_| invalid("JP2 box length exceeds usize"))?,
                )
                .ok_or_else(|| invalid("JP2 box length overflow"))?,
        };
        if end > self.bytes.len() || end < self.pos {
            return Err(invalid("JP2 box extends past input"));
        }
        let payload = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(Some(BoxRecord { box_type, payload }))
    }
}

fn read_box_type(bytes: &[u8], offset: usize) -> Result<[u8; 4]> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("unexpected end of JP2 box type"))?;
    Ok([slice[0], slice[1], slice[2], slice[3]])
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| invalid("unexpected end of JP2 u16"))?;
    Ok(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("unexpected end of JP2 u32"))?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| invalid("unexpected end of JP2 u64"))?;
    Ok(u64::from_be_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
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

    #[test]
    fn bpcc_box_fails_fast_with_feature_name() {
        let bytes = jp2_with_extra_header_box(BOX_BITS_PER_COMPONENT, &[7]);

        let err = parse_jp2(&bytes).expect_err("bpcc should be rejected").to_string();

        assert!(err.contains("per-component bit-depth box (bpcc)"), "{err}");
    }

    #[test]
    fn palette_box_fails_fast_with_feature_name() {
        let bytes = jp2_with_extra_header_box(BOX_PALETTE, &[0, 0, 0, 0]);

        let err = parse_jp2(&bytes).expect_err("pclr should be rejected").to_string();

        assert!(err.contains("palette box (pclr)"), "{err}");
    }

    #[test]
    fn component_mapping_box_fails_fast_with_feature_name() {
        let bytes = jp2_with_extra_header_box(BOX_COMPONENT_MAPPING, &[0, 0, 0, 0]);

        let err = parse_jp2(&bytes).expect_err("cmap should be rejected").to_string();

        assert!(err.contains("component mapping box (cmap)"), "{err}");
    }

    #[test]
    fn channel_definition_box_fails_fast_with_feature_name() {
        let bytes = jp2_with_extra_header_box(BOX_CHANNEL_DEFINITION, &[0, 0, 0, 0]);

        let err = parse_jp2(&bytes).expect_err("cdef should be rejected").to_string();

        assert!(err.contains("channel definition box (cdef)"), "{err}");
    }

    #[test]
    fn icc_profile_colr_fails_fast() {
        let bytes = jp2_with_colr_payload(&[2, 0, 0, 0]);

        let err = parse_jp2(&bytes).expect_err("ICC colr should be rejected").to_string();

        assert!(err.contains("only enumerated JP2 colorspaces are supported"), "{err}");
    }

    #[test]
    fn multiple_codestream_boxes_fail_fast() {
        let mut bytes = minimal_jp2_header();
        push_box(&mut bytes, BOX_CODESTREAM, &[0xff, 0x4f]);
        push_box(&mut bytes, BOX_CODESTREAM, &[0xff, 0x4f]);

        let err = parse_jp2(&bytes)
            .expect_err("multiple jp2c boxes should be rejected")
            .to_string();

        assert!(err.contains("multiple contiguous codestream boxes"), "{err}");
    }

    fn jp2_with_extra_header_box(box_type: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        push_box(&mut header, BOX_IMAGE_HEADER, &ihdr_payload());
        push_box(&mut header, box_type, payload);
        push_box(&mut header, BOX_COLOR_SPEC, &enumerated_colr_payload(ENUM_GRAY));
        wrap_with_header_and_codestream(header)
    }

    fn jp2_with_colr_payload(colr: &[u8]) -> Vec<u8> {
        let mut header = Vec::new();
        push_box(&mut header, BOX_IMAGE_HEADER, &ihdr_payload());
        push_box(&mut header, BOX_COLOR_SPEC, colr);
        wrap_with_header_and_codestream(header)
    }

    fn wrap_with_header_and_codestream(header_payload: Vec<u8>) -> Vec<u8> {
        let mut bytes = minimal_jp2_header();
        push_box(&mut bytes, BOX_JP2_HEADER, &header_payload);
        push_box(&mut bytes, BOX_CODESTREAM, &[0xff, 0x4f]);
        bytes
    }

    fn minimal_jp2_header() -> Vec<u8> {
        let mut bytes = Vec::new();
        push_box(&mut bytes, BOX_SIGNATURE, &JP2_SIGNATURE_PAYLOAD);
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(b"jp2 ");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        ftyp.extend_from_slice(b"jp2 ");
        push_box(&mut bytes, BOX_FILE_TYPE, &ftyp);
        bytes
    }

    fn ihdr_payload() -> [u8; 14] {
        let mut payload = [0u8; 14];
        payload[0..4].copy_from_slice(&8u32.to_be_bytes());
        payload[4..8].copy_from_slice(&8u32.to_be_bytes());
        payload[8..10].copy_from_slice(&1u16.to_be_bytes());
        payload[10] = 7;
        payload[11] = JP2_COMPRESSION_TYPE_J2K;
        payload
    }

    fn enumerated_colr_payload(enum_cs: u32) -> [u8; 7] {
        let mut payload = [0u8; 7];
        payload[0] = 1;
        payload[3..7].copy_from_slice(&enum_cs.to_be_bytes());
        payload
    }

    fn push_box(out: &mut Vec<u8>, box_type: [u8; 4], payload: &[u8]) {
        let len = u32::try_from(payload.len() + 8).expect("box length");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&box_type);
        out.extend_from_slice(payload);
    }
}
