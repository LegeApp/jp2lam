//! Narrow JPEG 2000 decoder entry points.
//!
//! This module starts with the Annex I JP2 container and Annex A codestream
//! header slice of the decoder plan. Later Tier-2 and Tier-1 stages should
//! consume these typed headers rather than reparsing marker bytes.

mod codestream;
mod jp2_parse;
mod reconstruct;
pub(crate) mod t1;
pub(crate) mod t2;

use crate::error::Result;
use crate::model::{ColorSpace, Image};
use std::io::Read;

pub use crate::j2k::decode_markers::{
    CodeBlockStyle, CodSegment, CodestreamHeader, ComponentSiz, PrecinctSize, ProgressionOrder,
    QcdSegment, QuantizationStep, QuantizationStyle, SizSegment, WaveletTransform,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeMetadata {
    pub width: u32,
    pub height: u32,
    pub colorspace: ColorSpace,
    pub has_ipr_metadata: bool,
    pub codestream: CodestreamHeader,
    pub tile_part_count: usize,
    pub first_tile_payload_len: usize,
}

/// Parse the JP2 wrapper and JPEG 2000 Part 1 main header.
///
/// This is the first tactical decoder slice: it validates the container,
/// extracts the first `jp2c` codestream box, and decodes the SIZ/COD/QCD marker
/// segments needed by packet and Tier-1 decoding.
pub fn inspect_jp2(bytes: &[u8]) -> Result<DecodeMetadata> {
    let parsed = jp2_parse::parse_jp2(bytes)?;
    let parts = codestream::parse_codestream_view(parsed.codestream)?;
    let first_tile = parts
        .tile_parts
        .first()
        .ok_or_else(|| crate::Jp2LamError::DecodeFailed("codestream has no tile-part".into()))?;
    let codestream = CodestreamHeader::from_marker_segments_with_tile_headers(
        parts.main_header_segments.iter().copied(),
        first_tile.header_segments.iter().copied(),
        first_tile.header,
        parts.tile_parts.len(),
    )?;
    validate_jp2_decode_scope(&parsed.header, &codestream)?;
    Ok(DecodeMetadata {
        width: parsed.header.width,
        height: parsed.header.height,
        colorspace: parsed.header.colorspace,
        has_ipr_metadata: parsed.header.has_ipr_metadata,
        first_tile_payload_len: parts
            .tile_parts
            .first()
            .map(|tile| tile.payload.len())
            .unwrap_or(0),
        tile_part_count: parts.tile_parts.len(),
        codestream,
    })
}

/// Decode a narrow grayscale JP2 image into the crate's native [`Image`] model.
///
/// This currently targets the Part 1 single-tile Archive.org-style page images
/// accepted by [`inspect_jp2`]: unsigned 8-bit grayscale or sRGB, LRCP
/// progression, no precinct/SOP/EPH syntax, and default MQ code-block style.
pub fn decode_jp2(bytes: &[u8]) -> Result<Image> {
    let parsed = jp2_parse::parse_jp2(bytes)?;
    let parts = codestream::parse_codestream_view(parsed.codestream)?;
    let first_tile = parts
        .tile_parts
        .first()
        .ok_or_else(|| crate::Jp2LamError::DecodeFailed("codestream has no tile-part".into()))?;
    let codestream = CodestreamHeader::from_marker_segments_with_tile_headers(
        parts.main_header_segments.iter().copied(),
        first_tile.header_segments.iter().copied(),
        first_tile.header,
        parts.tile_parts.len(),
    )?;
    validate_jp2_decode_scope(&parsed.header, &codestream)?;
    let packets = t2::parse_tile_part_payload(&codestream, first_tile.payload)?;
    let components = t1::decode_tile_components(&codestream, &packets)?;
    reconstruct::reconstruct_image(&codestream, parsed.header.colorspace, components)
}

fn validate_jp2_decode_scope(
    header: &jp2_parse::Jp2Header,
    codestream: &CodestreamHeader,
) -> Result<()> {
    if header.width != codestream.siz.width || header.height != codestream.siz.height {
        return Err(crate::Jp2LamError::DecodeFailed(format!(
            "JP2 ihdr dimensions {}x{} do not match SIZ dimensions {}x{}",
            header.width, header.height, codestream.siz.width, codestream.siz.height
        )));
    }
    if header.bits_per_component != 8 {
        return Err(crate::Jp2LamError::UnsupportedFeature(format!(
            "unsupported JP2 bit depth: decoder currently supports 8-bit components, found {} bits",
            header.bits_per_component
        )));
    }
    if header.component_count != codestream.siz.components.len() as u16 {
        return Err(crate::Jp2LamError::DecodeFailed(format!(
            "JP2 ihdr component count {} does not match SIZ component count {}",
            header.component_count,
            codestream.siz.components.len()
        )));
    }
    if header.colorspace == ColorSpace::Gray && header.component_count != 1 {
        return Err(crate::Jp2LamError::UnsupportedFeature(format!(
            "unsupported JP2 component count: decoder currently supports one grayscale component, found {} components",
            header.component_count
        )));
    }
    if header.colorspace == ColorSpace::Srgb && header.component_count != 3 {
        return Err(crate::Jp2LamError::UnsupportedFeature(format!(
            "unsupported JP2 component count: decoder currently supports three sRGB components, found {} components",
            header.component_count
        )));
    }
    if header.colorspace != ColorSpace::Gray && header.colorspace != ColorSpace::Srgb {
        return Err(crate::Jp2LamError::UnsupportedFeature(format!(
            "unsupported JP2 colorspace: decoder currently supports EnumCS=17 grayscale and EnumCS=16 sRGB, found {:?}",
            header.colorspace
        )));
    }
    Ok(())
}

/// Read a complete JP2 stream from memory-backed or file-backed input and
/// decode it into an [`Image`].
///
/// Prefer [`decode_jp2`] when the bytes are already available; it borrows the
/// input buffer through JP2 parsing, codestream framing, and Tier-2 packet
/// parsing instead of copying tile payload bytes.
pub fn decode_from_reader<R: Read>(reader: &mut R) -> Result<Image> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|err| crate::Jp2LamError::Io(format!("failed to read JP2 input: {err}")))?;
    decode_jp2(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_org_sample_header_matches_decoder_scope() {
        let bytes = read_archive_org_sample();
        let metadata = inspect_jp2(&bytes).expect("inspect sample jp2");

        assert_eq!(metadata.width, 3494);
        assert_eq!(metadata.height, 4967);
        assert_eq!(metadata.colorspace, ColorSpace::Gray);
        assert!(!metadata.has_ipr_metadata);
        assert_eq!(metadata.tile_part_count, 1);
        assert!(metadata.first_tile_payload_len > 1_000_000);

        let siz = &metadata.codestream.siz;
        assert_eq!(siz.width, 3494);
        assert_eq!(siz.height, 4967);
        assert_eq!(siz.tile_width, 3494);
        assert_eq!(siz.tile_height, 4967);
        assert_eq!(siz.components.len(), 1);
        assert_eq!(siz.components[0].precision, 8);
        assert!(!siz.components[0].signed);
        assert_eq!(siz.components[0].dx, 1);
        assert_eq!(siz.components[0].dy, 1);

        let cod = &metadata.codestream.cod;
        assert_eq!(cod.progression_order, ProgressionOrder::Lrcp);
        assert_eq!(cod.layers, 1);
        assert_eq!(cod.decomposition_levels, 5);
        assert_eq!(cod.code_block_width, 64);
        assert_eq!(cod.code_block_height, 64);
        assert_eq!(cod.code_block_style, CodeBlockStyle::default());
        assert_eq!(cod.transform, WaveletTransform::Irreversible97);
        assert!(!cod.uses_precincts);
        assert!(!cod.sop_markers);
        assert!(!cod.eph_markers);

        let qcd = &metadata.codestream.qcd;
        assert_eq!(qcd.style, QuantizationStyle::ScalarExpounded);
        assert_eq!(qcd.guard_bits, 1);
        assert_eq!(qcd.steps.len(), 16);
        assert_eq!(metadata.codestream.comment_count, 2);
    }

    #[test]
    fn archive_org_rgb_sample_header_matches_decoder_scope() {
        let bytes = read_archive_org_rgb_sample();
        let metadata = inspect_jp2(&bytes).expect("inspect rgb sample jp2");

        assert_eq!(metadata.width, 6000);
        assert_eq!(metadata.height, 4000);
        assert_eq!(metadata.colorspace, ColorSpace::Srgb);
        assert_eq!(metadata.tile_part_count, 1);

        let siz = &metadata.codestream.siz;
        assert_eq!(siz.components.len(), 3);
        for component in &siz.components {
            assert_eq!(component.precision, 8);
            assert!(!component.signed);
            assert_eq!(component.dx, 1);
            assert_eq!(component.dy, 1);
        }

        let cod = &metadata.codestream.cod;
        assert_eq!(cod.progression_order, ProgressionOrder::Lrcp);
        assert_eq!(cod.layers, 1);
        assert!(cod.use_mct);
        assert_eq!(cod.decomposition_levels, 5);
        assert_eq!(cod.code_block_width, 64);
        assert_eq!(cod.code_block_height, 64);
        assert_eq!(cod.code_block_style, CodeBlockStyle::default());
        assert_eq!(cod.transform, WaveletTransform::Irreversible97);
        assert!(!cod.uses_precincts);

        let qcd = &metadata.codestream.qcd;
        assert_eq!(qcd.style, QuantizationStyle::ScalarExpounded);
        assert_eq!(qcd.guard_bits, 1);
        assert_eq!(qcd.steps.len(), 16);
    }

    #[test]
    fn inspect_jp2_rejects_jp2_codestream_component_mismatch() {
        let mut bytes = read_archive_org_sample();
        let ihdr = find_box_payload(&bytes, b"ihdr").expect("find ihdr");
        bytes[ihdr + 8..ihdr + 10].copy_from_slice(&2u16.to_be_bytes());

        let err = inspect_jp2(&bytes)
            .expect_err("JP2/SIZ component mismatch should fail during inspection")
            .to_string();

        assert!(err.contains("JP2 ihdr component count"), "{err}");
    }

    #[test]
    fn inspect_jp2_rejects_jp2_codestream_dimension_mismatch() {
        let mut bytes = read_archive_org_sample();
        let ihdr = find_box_payload(&bytes, b"ihdr").expect("find ihdr");
        bytes[ihdr + 4..ihdr + 8].copy_from_slice(&1234u32.to_be_bytes());

        let err = inspect_jp2(&bytes)
            .expect_err("JP2/SIZ dimension mismatch should fail during inspection")
            .to_string();

        assert!(err.contains("JP2 ihdr dimensions"), "{err}");
    }

    #[test]
    fn decode_jp2_rejects_truncated_tile_payload_before_image_output() {
        let bytes = read_archive_org_sample();
        let truncated = bytes[..bytes.len() - 4096].to_vec();

        let err = decode_jp2(&truncated)
            .expect_err("truncated JP2 should fail before reconstruction");

        assert!(matches!(err, crate::Jp2LamError::DecodeFailed(_)), "{err:?}");
        let err = err.to_string();
        assert!(
            err.contains("packet body extends past tile payload")
                || err.contains("tile payload has")
                || err.contains("tile-part length exceeded codestream size")
                || err.contains("EOC"),
            "{err}"
        );
    }

    #[test]
    fn decode_errors_expose_matchable_unsupported_feature_variant() {
        let mut bytes = read_archive_org_sample();
        let ihdr = find_box_payload(&bytes, b"ihdr").expect("find ihdr");
        bytes[ihdr + 10] = 15;

        let err = inspect_jp2(&bytes)
            .expect_err("unsupported JP2 bit depth should be matchable");

        assert!(err.is_decode_failure());
        assert!(err.is_unsupported_feature());
        assert!(matches!(err, crate::Jp2LamError::UnsupportedFeature(_)), "{err:?}");
        assert!(err.message().contains("bit depth"));
    }

    #[test]
    #[ignore = "scans the full provided Archive.org RGB JP2 directory"]
    fn archive_org_rgb_directory_headers_match_decoder_scope() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/moreboysgirlsofh0000rhod_e0h1_orig_jp2"
        );
        let mut count = 0usize;
        for entry in std::fs::read_dir(dir).expect("read rgb jp2 directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jp2") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read rgb jp2 page");
            let metadata = inspect_jp2(&bytes)
                .unwrap_or_else(|err| panic!("inspect {}: {err}", path.display()));
            assert_eq!(metadata.colorspace, ColorSpace::Srgb, "{}", path.display());
            assert_eq!(metadata.codestream.siz.components.len(), 3, "{}", path.display());
            assert_eq!(metadata.codestream.cod.progression_order, ProgressionOrder::Lrcp, "{}", path.display());
            assert_eq!(metadata.codestream.cod.layers, 1, "{}", path.display());
            assert!(metadata.codestream.cod.use_mct, "{}", path.display());
            assert_eq!(metadata.codestream.cod.transform, WaveletTransform::Irreversible97, "{}", path.display());
            assert_eq!(metadata.codestream.qcd.style, QuantizationStyle::ScalarExpounded, "{}", path.display());
            count += 1;
        }
        assert!(count > 0, "expected at least one RGB JP2 page");
    }

    #[test]
    #[ignore = "decodes representative full-size Archive.org RGB JP2 pages"]
    fn archive_org_rgb_representative_pages_decode() {
        for name in [
            "moreboysgirlsofh0000rhod_e0h1_orig_0000.jp2",
            "moreboysgirlsofh0000rhod_e0h1_orig_0138.jp2",
            "moreboysgirlsofh0000rhod_e0h1_orig_0291.jp2",
        ] {
            let path = archive_org_rgb_path(name);
            let bytes = std::fs::read(&path).expect("read rgb jp2 page");
            let image = decode_jp2(&bytes)
                .unwrap_or_else(|err| panic!("decode {}: {err}", path.display()));

            assert_eq!(image.width, 6000, "{}", path.display());
            assert_eq!(image.height, 4000, "{}", path.display());
            assert_eq!(image.colorspace, ColorSpace::Srgb, "{}", path.display());
            assert_eq!(image.components.len(), 3, "{}", path.display());
            assert_decoded_components_are_8bit_full_size(&image);
            assert!(image.components.iter().all(|component| {
                component.data.iter().any(|&sample| sample != 0)
                    && component.data.iter().any(|&sample| sample != 255)
            }));
        }
    }

    #[test]
    #[ignore = "decodes every provided Archive.org RGB JP2 page; expensive in debug builds"]
    fn archive_org_rgb_directory_decodes_all_pages() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/moreboysgirlsofh0000rhod_e0h1_orig_jp2"
        );
        let mut paths = std::fs::read_dir(dir)
            .expect("read rgb jp2 directory")
            .map(|entry| entry.expect("directory entry").path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jp2"))
            .collect::<Vec<_>>();
        paths.sort();

        let mut count = 0usize;
        for path in paths {
            let bytes = std::fs::read(&path).expect("read rgb jp2 page");
            let image = decode_jp2(&bytes)
                .unwrap_or_else(|err| panic!("decode {}: {err}", path.display()));
            assert_eq!(image.colorspace, ColorSpace::Srgb, "{}", path.display());
            assert_decoded_components_are_8bit_full_size(&image);
            count += 1;
        }
        assert!(count > 0, "expected at least one RGB JP2 page");
    }

    #[test]
    #[ignore = "requires ImageMagick; compares decoder output against an independent JP2 decoder"]
    fn archive_org_gray_crop_matches_imagemagick() {
        if !imagemagick_available() {
            eprintln!("skipping ImageMagick comparison because `magick` is not available");
            return;
        }

        let bytes = read_archive_org_sample();
        let image = decode_jp2(&bytes).expect("decode grayscale sample");
        let reference = imagemagick_raw_crop(
            &archive_org_gray_path(),
            RawCrop {
                x: 256,
                y: 256,
                width: 96,
                height: 96,
                channels: 1,
            },
        );

        assert_crop_close_to_reference(&image, 256, 256, 96, 96, &reference, 1, 1, 0.05);
    }

    #[test]
    #[ignore = "requires ImageMagick; compares decoder output against an independent JP2 decoder"]
    fn archive_org_rgb_crop_matches_imagemagick() {
        if !imagemagick_available() {
            eprintln!("skipping ImageMagick comparison because `magick` is not available");
            return;
        }

        let path = archive_org_rgb_path("moreboysgirlsofh0000rhod_e0h1_orig_0000.jp2");
        let bytes = std::fs::read(&path).expect("read rgb jp2 page");
        let image = decode_jp2(&bytes).expect("decode rgb sample");
        let reference = imagemagick_raw_crop(
            &path,
            RawCrop {
                x: 128,
                y: 128,
                width: 64,
                height: 64,
                channels: 3,
            },
        );

        assert_crop_close_to_reference(&image, 128, 128, 64, 64, &reference, 3, 3, 0.25);
    }

    #[test]
    fn archive_org_sample_packet_headers_split_tile_payload() {
        let bytes = read_archive_org_sample();
        let parsed = jp2_parse::parse_jp2(&bytes).expect("parse jp2");
        let parts = codestream::parse_codestream_view(parsed.codestream).expect("parse j2k");
        let codestream = CodestreamHeader::from_marker_segments_with_tile_headers(
            parts.main_header_segments.iter().copied(),
            parts.tile_parts[0].header_segments.iter().copied(),
            parts.tile_parts[0].header,
            parts.tile_parts.len(),
        )
        .expect("decode headers");
        let payload = parts.tile_parts[0].payload;
        let packets =
            t2::parse_tile_part_payload(&codestream, payload).expect("parse packet headers");

        assert_eq!(
            packets.packets.len(),
            codestream.cod.layers as usize
                * (usize::from(codestream.cod.decomposition_levels) + 1)
        );
        assert_eq!(
            packets
                .packets
                .iter()
                .map(|packet| packet.header_len + packet.body_len)
                .sum::<usize>(),
            payload.len()
        );
        assert!(!packets.codeblocks.is_empty());
        assert!(packets.codeblocks.iter().any(|block| block.passes > 0));
    }

    #[test]
    fn archive_org_sample_tier1_decodes_quantized_coefficients() {
        let bytes = read_archive_org_sample();
        let parsed = jp2_parse::parse_jp2(&bytes).expect("parse jp2");
        let parts = codestream::parse_codestream_view(parsed.codestream).expect("parse j2k");
        let codestream = CodestreamHeader::from_marker_segments_with_tile_headers(
            parts.main_header_segments.iter().copied(),
            parts.tile_parts[0].header_segments.iter().copied(),
            parts.tile_parts[0].header,
            parts.tile_parts.len(),
        )
        .expect("decode headers");
        let payload = parts.tile_parts[0].payload;
        let packets =
            t2::parse_tile_part_payload(&codestream, payload).expect("parse packet headers");
        let tile =
            t1::decode_tile_coefficients(&codestream, &packets).expect("decode tier1 blocks");

        assert_eq!(tile.width, 3494);
        assert_eq!(tile.height, 4967);
        assert_eq!(tile.coefficients.len(), 3494 * 4967);
        assert!(tile.coefficients.iter().any(|&coefficient| coefficient != 0));
    }

    #[test]
    fn archive_org_sample_reconstructs_grayscale_image() {
        let bytes = read_archive_org_sample();
        let image = decode_jp2(&bytes).expect("decode sample jp2");

        assert_eq!(image.width, 3494);
        assert_eq!(image.height, 4967);
        assert_eq!(image.colorspace, ColorSpace::Gray);
        assert_eq!(image.components.len(), 1);
        assert_eq!(image.components[0].data.len(), 3494 * 4967);
        assert!(
            image.components[0]
                .data
                .iter()
                .all(|&sample| (0..=255).contains(&sample))
        );
        assert!(image.components[0].data.iter().any(|&sample| sample != 0));
        assert!(image.components[0].data.iter().any(|&sample| sample != 255));
    }

    #[test]
    fn decode_jp2_roundtrips_native_gray_lossless() {
        let width = 32;
        let height = 32;
        let samples = (0..height)
            .flat_map(|y| (0..width).map(move |x| ((x * 7 + y * 11 + (x ^ y)) & 0xff) as u8))
            .collect::<Vec<_>>();
        let image = Image::from_gray_bytes(width, height, &samples).expect("source image");
        let encoded = crate::encode(
            &image,
            &crate::EncodeOptions {
                quality: 100,
                format: crate::OutputFormat::Jp2,
            },
        )
        .expect("encode jp2");

        let decoded = decode_jp2(&encoded).expect("decode jp2");
        assert_eq!(decoded.width, width);
        assert_eq!(decoded.height, height);
        assert_eq!(decoded.colorspace, ColorSpace::Gray);
        assert_eq!(decoded.components[0].data, image.components[0].data);
    }

    #[test]
    fn decode_from_reader_matches_slice_decode() {
        let bytes = read_archive_org_sample();
        let from_slice = decode_jp2(&bytes).expect("decode slice");
        let mut cursor = std::io::Cursor::new(&bytes);
        let from_reader = decode_from_reader(&mut cursor).expect("decode reader");

        assert_eq!(from_reader.width, from_slice.width);
        assert_eq!(from_reader.height, from_slice.height);
        assert_eq!(from_reader.colorspace, from_slice.colorspace);
        assert_eq!(from_reader.components[0].data, from_slice.components[0].data);
    }

    fn read_archive_org_sample() -> Vec<u8> {
        std::fs::read(archive_org_gray_path()).expect("read sample jp2")
    }

    fn read_archive_org_rgb_sample() -> Vec<u8> {
        std::fs::read(archive_org_rgb_path(
            "moreboysgirlsofh0000rhod_e0h1_orig_0000.jp2",
        ))
        .expect("read rgb sample jp2")
    }

    fn find_box_payload(bytes: &[u8], box_type: &[u8; 4]) -> Option<usize> {
        fn walk_boxes(bytes: &[u8], box_type: &[u8; 4]) -> Option<usize> {
            let mut pos = 0usize;
            while pos + 8 <= bytes.len() {
                let start = pos;
                let lbox = u32::from_be_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                ]) as usize;
                let current_type = &bytes[pos + 4..pos + 8];
                pos += 8;
                let (payload_start, end) = if lbox == 1 {
                    if pos + 8 > bytes.len() {
                        return None;
                    }
                    let xlbox = u64::from_be_bytes([
                        bytes[pos],
                        bytes[pos + 1],
                        bytes[pos + 2],
                        bytes[pos + 3],
                        bytes[pos + 4],
                        bytes[pos + 5],
                        bytes[pos + 6],
                        bytes[pos + 7],
                    ]) as usize;
                    pos += 8;
                    (pos, start.checked_add(xlbox)?)
                } else if lbox == 0 {
                    (pos, bytes.len())
                } else {
                    (pos, start.checked_add(lbox)?)
                };
                if end > bytes.len() || end < payload_start {
                    return None;
                }
                if current_type == box_type {
                    return Some(payload_start);
                }
                if current_type == b"jp2h" {
                    if let Some(found) = walk_boxes(&bytes[payload_start..end], box_type) {
                        return Some(payload_start + found);
                    }
                }
                pos = end;
            }
            None
        }

        walk_boxes(bytes, box_type)
    }

    fn archive_org_gray_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("2015.207614.Finnegans-Wake_0012.jp2")
    }

    fn archive_org_rgb_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("moreboysgirlsofh0000rhod_e0h1_orig_jp2")
            .join(name)
    }

    fn assert_decoded_components_are_8bit_full_size(image: &Image) {
        let pixel_count = image.width as usize * image.height as usize;
        for component in &image.components {
            assert_eq!(component.width, image.width);
            assert_eq!(component.height, image.height);
            assert_eq!(component.precision, 8);
            assert!(!component.signed);
            assert_eq!(component.dx, 1);
            assert_eq!(component.dy, 1);
            assert_eq!(component.data.len(), pixel_count);
            assert!(component.data.iter().all(|&sample| (0..=255).contains(&sample)));
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct RawCrop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        channels: usize,
    }

    fn imagemagick_available() -> bool {
        std::process::Command::new("magick")
            .arg("-version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn imagemagick_raw_crop(path: &std::path::Path, crop: RawCrop) -> Vec<u8> {
        let raw_path = std::env::temp_dir().join(format!(
            "jp2lam_ref_{}_{}_{}_{}.raw",
            std::process::id(),
            crop.x,
            crop.y,
            crop.channels
        ));
        let format = if crop.channels == 1 { "gray" } else { "rgb" };
        let output_arg = format!("{}:{}", format, raw_path.display());
        let status = std::process::Command::new("magick")
            .arg(path)
            .arg("-crop")
            .arg(format!("{}x{}+{}+{}", crop.width, crop.height, crop.x, crop.y))
            .arg("+repage")
            .arg("-depth")
            .arg("8")
            .arg(output_arg)
            .status()
            .expect("run ImageMagick");
        assert!(status.success(), "ImageMagick failed for {}", path.display());
        let bytes = std::fs::read(&raw_path).expect("read ImageMagick raw crop");
        let _ = std::fs::remove_file(raw_path);
        assert_eq!(
            bytes.len(),
            crop.width as usize * crop.height as usize * crop.channels
        );
        bytes
    }

    fn assert_crop_close_to_reference(
        image: &Image,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        reference: &[u8],
        channels: usize,
        max_abs_allowed: i32,
        mean_abs_allowed: f64,
    ) {
        assert_eq!(image.components.len(), channels);
        let mut max_abs = 0i32;
        let mut total_abs = 0u64;
        let mut count = 0u64;
        for yy in 0..height {
            for xx in 0..width {
                let src_idx = ((y + yy) * image.width + (x + xx)) as usize;
                let ref_idx = ((yy * width + xx) as usize) * channels;
                for channel in 0..channels {
                    let actual = image.components[channel].data[src_idx];
                    let expected = i32::from(reference[ref_idx + channel]);
                    let delta = (actual - expected).abs();
                    max_abs = max_abs.max(delta);
                    total_abs += delta as u64;
                    count += 1;
                }
            }
        }
        let mean_abs = total_abs as f64 / count as f64;
        assert!(
            max_abs <= max_abs_allowed,
            "max abs diff {max_abs} exceeded {max_abs_allowed}"
        );
        assert!(
            mean_abs <= mean_abs_allowed,
            "mean abs diff {mean_abs:.4} exceeded {mean_abs_allowed:.4}"
        );
    }
}
