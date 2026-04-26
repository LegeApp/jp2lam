use crate::error::{Jp2LamError, Result};
use crate::model::{ColorSpace, Image};

const JP2_SIGNATURE_BOX_LEN: u32 = 12;
const JP2_FILE_TYPE_BOX_LEN: u32 = 20;
const JP2_IMAGE_HEADER_BOX_LEN: u32 = 22;
const JP2_COLOR_SPEC_BOX_LEN: u32 = 15;
const JP2_HEADER_BOX_LEN: u32 = 8 + JP2_IMAGE_HEADER_BOX_LEN + JP2_COLOR_SPEC_BOX_LEN;
const JP2_ENUM_SRGB: u32 = 16;
const JP2_ENUM_GRAY: u32 = 17;
const JP2_COMPRESSION_TYPE_J2K: u8 = 7;

pub(crate) fn wrap_codestream(image: &Image, codestream: &[u8]) -> Result<Vec<u8>> {
    let codestream_box_len = box_len(codestream.len())?;
    let total_len = JP2_SIGNATURE_BOX_LEN as usize
        + JP2_FILE_TYPE_BOX_LEN as usize
        + JP2_HEADER_BOX_LEN as usize
        + codestream_box_len as usize;
    let mut out = Vec::with_capacity(total_len);

    write_box_header(&mut out, JP2_SIGNATURE_BOX_LEN, b"jP  ");
    out.extend_from_slice(&[0x0d, 0x0a, 0x87, 0x0a]);

    write_box_header(&mut out, JP2_FILE_TYPE_BOX_LEN, b"ftyp");
    out.extend_from_slice(b"jp2 ");
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"jp2 ");

    write_box_header(&mut out, JP2_HEADER_BOX_LEN, b"jp2h");

    write_box_header(&mut out, JP2_IMAGE_HEADER_BOX_LEN, b"ihdr");
    out.extend_from_slice(&image.height.to_be_bytes());
    out.extend_from_slice(&image.width.to_be_bytes());
    out.extend_from_slice(&(image.components.len() as u16).to_be_bytes());
    out.push(7);
    out.push(JP2_COMPRESSION_TYPE_J2K);
    out.push(0);
    out.push(0);

    write_box_header(&mut out, JP2_COLOR_SPEC_BOX_LEN, b"colr");
    out.push(1);
    out.push(0);
    out.push(0);
    out.extend_from_slice(&enumcs(image.colorspace).to_be_bytes());

    write_box_header(&mut out, codestream_box_len, b"jp2c");
    out.extend_from_slice(codestream);

    Ok(out)
}

fn enumcs(color_space: ColorSpace) -> u32 {
    match color_space.encoding_domain() {
        ColorSpace::Gray => JP2_ENUM_GRAY,
        ColorSpace::Srgb => JP2_ENUM_SRGB,
        _ => JP2_ENUM_SRGB,
    }
}

fn box_len(payload_len: usize) -> Result<u32> {
    let total_len = payload_len
        .checked_add(8)
        .ok_or_else(|| Jp2LamError::EncodeFailed("JP2 box length overflow".to_string()))?;
    u32::try_from(total_len)
        .map_err(|_| Jp2LamError::EncodeFailed("JP2 box length exceeds 32-bit limit".to_string()))
}

fn write_box_header(out: &mut Vec<u8>, len: u32, box_type: &[u8; 4]) {
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(box_type);
}
