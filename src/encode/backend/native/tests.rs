use super::{NativeBackend, NativeComponentCoefficients};
use crate::encode::backend::{CodestreamBackend, OpenJp2Backend};
use crate::encode::context::EncodeContext;
use crate::j2k::CodestreamParts;
use crate::model::{ColorSpace, Component, EncodeOptions, Image, OutputFormat, Preset};
use crate::plan::BandOrientation;
use openjp2::{opj_dparameters_t, Codec, Stream, CODEC_FORMAT};

#[test]
fn native_backend_prepares_gray_lossless_coefficients() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let prepared = NativeBackend
        .prepare_component_coefficients(&context, 0)
        .expect("prepare coefficients");

    assert_eq!(
        prepared,
        NativeComponentCoefficients {
            width: 4,
            height: 4,
            levels: context.plan.decomposition_levels,
            data: vec![-38, 36, 0, 16, 144, 0, 0, 16, 0, 0, 0, 0, 64, 64, 0, 0],
        }
    );
}

#[cfg(all(test, feature = "openjp2-oracle"))]
#[test]
fn compare_rgb_lossy_headers_minimal() {
    let image = Image {
        width: 8, height: 8,
        components: vec![
            Component { data: vec![200; 64], width: 8, height: 8, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: vec![100; 64], width: 8, height: 8, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: vec![50; 64], width: 8, height: 8, precision: 8, signed: false, dx: 1, dy: 1 },
        ],
        colorspace: ColorSpace::Srgb,
    };
    let context = EncodeContext::new(&image, &EncodeOptions { preset: Preset::WebHigh, format: OutputFormat::J2k }).expect("build");

    let native = NativeBackend.encode_codestream(&context).expect("native");
    let openjp2 = OpenJp2Backend.encode_codestream(&context).expect("openjp2");
    
    eprintln!("Native: {} bytes, OpenJP2: {} bytes", native.len(), openjp2.len());
}

#[cfg(all(test, feature = "openjp2-oracle"))]
#[test]
fn native_backend_rgb_lossy_quants_per_component() {
    let image = Image {
        width: 8,
        height: 8,
        components: vec![
            Component { data: vec![128; 64], width: 8, height: 8, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: vec![128; 64], width: 8, height: 8, precision: 8, signed: false, dx: 1, dy: 1 },
            Component { data: vec![128; 64], width: 8, height: 8, precision: 8, signed: false, dx: 1, dy: 1 },
        ],
        colorspace: ColorSpace::Srgb,
    };
    let context = EncodeContext::new(
        &image,
        &EncodeOptions { preset: Preset::WebHigh, format: OutputFormat::J2k },
    ).expect("build");

    let native = NativeBackend.encode_codestream(&context).expect("native");
    let openjp2 = OpenJp2Backend.encode_codestream(&context).expect("openjp2");
    
    // Find SOT marker (start of first tile part) 
    let native_sot = native.iter().position(|&b| b == 0xff).and_then(|i| 
        if i + 1 < native.len() && native[i+1] == 0x90 { Some(i) } else { None }
    ).unwrap_or(0);
    let openjp2_sot = openjp2.iter().position(|&b| b == 0xff).and_then(|i|
        if i + 1 < openjp2.len() && openjp2[i+1] == 0x90 { Some(i) } else { None }
    ).unwrap_or(0);
    
    // Main header up to SOD/tile data
    assert_eq!(native_sot, openjp2_sot, "tile start differs");
    assert_eq!(&native[..native_sot], &openjp2[..openjp2_sot], "main headers differ");
    
    // Total sizes
    assert_eq!(native.len(), openjp2.len(), "total sizes differ: native={}, openjp2={}", native.len(), openjp2.len());
}

#[test]
fn native_backend_recognizes_gray_lossless_lane() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    assert!(NativeBackend.supports_lane(&context));
    assert!(NativeBackend.supports(&context));
}

#[test]
fn native_backend_builds_gray_lossless_layout() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let layout = NativeBackend
        .prepare_component_layout(&context, 0)
        .expect("build native layout");

    assert_eq!(layout.subbands.len(), 7);
    assert_eq!(layout.subbands[0].band, BandOrientation::Ll);
    assert_eq!(layout.subbands[0].codeblocks.len(), 1);
    assert_eq!(layout.subbands[0].codeblocks[0].coefficients, vec![-38]);
    let hh2 = layout
        .subbands
        .iter()
        .find(|band| band.resolution == 2 && band.band == BandOrientation::Hh)
        .expect("resolution 2 hh band");
    assert_eq!(hh2.codeblocks[0].coefficients, vec![0, 0, 0, 0]);
}

#[test]
fn native_backend_builds_gray_lossless_tier1_layout() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let analyzed = NativeBackend
        .prepare_tier1_layout(&context, 0)
        .expect("build native tier1 layout");

    assert_eq!(analyzed.bands.len(), 7);
    let ll = analyzed
        .bands
        .iter()
        .find(|band| band.resolution == 0 && band.band == BandOrientation::Ll)
        .expect("ll band");
    assert_eq!(ll.blocks[0].max_magnitude, 38);
    assert_eq!(ll.blocks[0].magnitude_bitplanes, 6);
    assert_eq!(ll.blocks[0].coding_passes.len(), 16);
    let hh2 = analyzed
        .bands
        .iter()
        .find(|band| band.resolution == 2 && band.band == BandOrientation::Hh)
        .expect("resolution 2 hh band");
    assert_eq!(hh2.blocks[0].max_magnitude, 0);
    assert!(hh2.blocks[0].coding_passes.is_empty());
}

#[test]
fn native_backend_builds_gray_lossless_encoded_tier1_layout() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let encoded = NativeBackend
        .prepare_tier1_encoded_layout(&context)
        .expect("build native encoded tier1 layout");

    let ll = encoded
        .bands
        .iter()
        .find(|band| band.resolution == 0 && band.band == BandOrientation::Ll)
        .expect("ll band");
    assert_eq!(ll.blocks[0].passes.len(), 16);
    assert!(!ll.blocks[0].passes.last().unwrap().bytes.is_empty());
    assert_eq!(
        ll.blocks[0].passes[0].cumulative_length,
        ll.blocks[0].passes[0].length
    );
    assert_eq!(
        ll.blocks[0].passes[15].cumulative_length,
        ll.blocks[0]
            .passes
            .iter()
            .map(|pass| pass.length)
            .sum::<usize>()
    );
}

#[test]
fn native_backend_builds_gray_lossless_packet_sequence() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let packets = NativeBackend
        .prepare_packet_sequence(&context)
        .expect("build native packet sequence");

    assert_eq!(packets.packets.len(), 3);
    assert_eq!(packets.packets[0].resolution, 0);
    assert_eq!(packets.packets[0].pass_count, 16);
    assert_eq!(packets.packets[1].codeblock_count, 3);
}

#[test]
fn native_backend_builds_gray_lossless_tile_part_payload() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let payload = NativeBackend
        .prepare_tile_part_payload(&context)
        .expect("build native tile part payload");
    let mut out = Vec::new();
    payload.write_to(&mut out);
    assert_eq!(payload.packet_count(), 3);
    assert!(!out.is_empty());
    assert_eq!(out[0] & 0x80, 0x80);
    assert!(out.len() > 8);
}

#[test]
fn native_backend_builds_gray_lossless_codestream_parts() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let parts = NativeBackend
        .prepare_codestream_parts(&context)
        .expect("build native codestream parts");
    assert_eq!(parts.main_header_segments.len(), 3);
    assert_eq!(parts.tile_parts.len(), 1);
    assert_eq!(parts.tile_parts[0].payload.packet_count(), 3);
    assert_eq!(parts.tile_parts[0].header.total_parts, 1);
}

#[test]
fn native_backend_builds_gray_lossless_codestream_bytes() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let bytes = NativeBackend
        .prepare_codestream_bytes(&context)
        .expect("build native codestream bytes");
    assert_eq!(&bytes[0..2], &[0xff, 0x4f]);
    assert_eq!(&bytes[2..4], &[0xff, 0x51]);
    assert_eq!(&bytes[bytes.len() - 2..], &[0xff, 0xd9]);

    let reparsed = CodestreamParts::parse_single_tile(&bytes).expect("parse native codestream");
    assert_eq!(reparsed.tile_parts.len(), 1);
    assert_eq!(reparsed.tile_parts[0].payload.packet_count(), 1);
    assert!(reparsed.tile_parts[0].payload.byte_len() > 0);
}

#[test]
fn native_backend_encode_codestream_matches_prepared_bytes() {
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
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let direct = NativeBackend
        .prepare_codestream_bytes(&context)
        .expect("build native codestream bytes");
    let via_trait = NativeBackend
        .encode_codestream(&context)
        .expect("encode native codestream");

    assert_eq!(via_trait, direct);
}

#[test]
fn tiny_gray_lossless_main_headers_match_openjp2() {
    // OpenJPEG rejects 1x1 grayscale-lossless in this path; the smallest
    // accepted oracle case is 2x2.
    let image = gray_image(2, 2, vec![0, 0, 0, 0]);
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let native = parse_backend_codestream(&NativeBackend, &context);
    let reference = parse_backend_codestream(&OpenJp2Backend, &context);

    // Ignore OpenJPEG's COM "Created by ..." marker which native doesn't emit.
    let strip_com = |segs: &[Vec<u8>]| -> Vec<Vec<u8>> {
        segs.iter()
            .filter(|s| s.len() < 2 || u16::from_be_bytes([s[0], s[1]]) != 0xff64)
            .cloned()
            .collect()
    };
    assert_eq!(
        strip_com(&native.main_header_segments),
        strip_com(&reference.main_header_segments)
    );
    assert_eq!(native.tile_parts.len(), 1);
    assert_eq!(reference.tile_parts.len(), 1);
    assert_eq!(native.tile_parts[0].header, reference.tile_parts[0].header);
}

#[test]
fn tiny_gray_lossless_payload_is_comparable_to_openjp2() {
    let image = gray_image(2, 2, vec![0, 0, 0, 0]);
    let context = EncodeContext::new(
        &image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let native = parse_backend_codestream(&NativeBackend, &context);
    let reference = parse_backend_codestream(&OpenJp2Backend, &context);
    let native_payload = payload_bytes(&native);
    let reference_payload = payload_bytes(&reference);

    assert!(!native_payload.is_empty());
    assert!(!reference_payload.is_empty());
    assert_eq!(native.tile_parts[0].payload.packet_count(), 1);
    assert_eq!(reference.tile_parts[0].payload.packet_count(), 1);
}

#[test]
fn parity_zero_gray_lossless_codestream_matches_openjp2() {
    let image = gray_image(2, 2, vec![0, 0, 0, 0]);
    assert_backend_parity(&image);
}

#[test]
fn parity_gradient_gray_lossless_codestream_matches_openjp2() {
    let image = gray_image(
        4,
        4,
        vec![
            0, 16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240,
        ],
    );
    assert_backend_parity(&image);
}

#[test]
fn native_gray_lossless_parity_cases_decode_exact_via_openjp2() {
    for image in [
        gray_image(2, 2, vec![0, 0, 0, 0]),
        gray_image(
            4,
            4,
            vec![
                0, 16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240,
            ],
        ),
    ] {
        assert_native_exact_roundtrip_via_openjp2("parity_case", &image);
    }
}

#[test]
fn native_gray_lossless_acceptance_corpus_decodes_exact_via_openjp2() {
    for (name, image) in [
        ("gradient_8x8", gray_gradient(8, 8)),
        ("gradient_17x19", gray_gradient(17, 19)),
        ("xor_8x8", gray_xor_pattern(8, 8)),
        ("xor_31x29", gray_xor_pattern(31, 29)),
    ] {
        assert_native_exact_roundtrip_via_openjp2(name, &image);
    }
}

fn gray_image(width: u32, height: u32, data: Vec<i32>) -> Image {
    Image {
        width,
        height,
        components: vec![Component {
            data,
            width,
            height,
            precision: 8,
            signed: false,
            dx: 1,
            dy: 1,
        }],
        colorspace: ColorSpace::Gray,
    }
}

fn gray_gradient(width: u32, height: u32) -> Image {
    let mut data = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            data.push(((x + y) % 256) as i32);
        }
    }
    gray_image(width, height, data)
}

fn gray_xor_pattern(width: u32, height: u32) -> Image {
    let mut data = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            data.push((((x ^ y) * 7) % 256) as i32);
        }
    }
    gray_image(width, height, data)
}

fn parse_backend_codestream(
    backend: &dyn CodestreamBackend,
    context: &EncodeContext<'_>,
) -> CodestreamParts {
    let bytes = backend
        .encode_codestream(context)
        .expect("encode backend codestream");
    CodestreamParts::parse_single_tile(&bytes).expect("parse backend codestream")
}

fn payload_bytes(parts: &CodestreamParts) -> Vec<u8> {
    let mut out = Vec::new();
    for tile_part in &parts.tile_parts {
        tile_part.payload.write_to(&mut out);
    }
    out
}

fn first_byte_mismatch(left: &[u8], right: &[u8]) -> Option<(usize, u8, u8)> {
    left.iter()
        .zip(right.iter())
        .enumerate()
        .find_map(|(idx, (&lhs, &rhs))| (lhs != rhs).then_some((idx, lhs, rhs)))
        .or_else(|| {
            if left.len() != right.len() {
                let idx = left.len().min(right.len());
                Some((
                    idx,
                    *left.get(idx).unwrap_or(&0),
                    *right.get(idx).unwrap_or(&0),
                ))
            } else {
                None
            }
        })
}

fn segment_markers(segments: &[Vec<u8>]) -> Vec<u16> {
    segments
        .iter()
        .filter_map(|segment| {
            (segment.len() >= 2).then_some(u16::from_be_bytes([segment[0], segment[1]]))
        })
        .collect()
}

fn substantive_segment_markers(segments: &[Vec<u8>]) -> Vec<u16> {
    segment_markers(segments)
        .into_iter()
        .filter(|&marker| marker != 0xff64)
        .collect()
}

fn assert_backend_parity(image: &Image) {
    let context = EncodeContext::new(
        image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context");

    let native_bytes = NativeBackend
        .encode_codestream(&context)
        .expect("encode native codestream");
    let reference_bytes = OpenJp2Backend
        .encode_codestream(&context)
        .expect("encode openjp2 codestream");

    let native_parts =
        CodestreamParts::parse_single_tile(&native_bytes).expect("parse native codestream");
    let reference_parts =
        CodestreamParts::parse_single_tile(&reference_bytes).expect("parse openjp2 codestream");

    let native_main = substantive_segment_markers(&native_parts.main_header_segments);
    let reference_main = substantive_segment_markers(&reference_parts.main_header_segments);
    let native_tile_headers = native_parts
        .tile_parts
        .iter()
        .map(|part| segment_markers(&part.header_segments))
        .collect::<Vec<_>>();
    let reference_tile_headers = reference_parts
        .tile_parts
        .iter()
        .map(|part| segment_markers(&part.header_segments))
        .collect::<Vec<_>>();

    if native_main != reference_main || native_tile_headers != reference_tile_headers {
        panic!(
                "codestream structure mismatch: main markers native={native_main:04x?}, openjp2={reference_main:04x?}; tile header markers native={native_tile_headers:04x?}, openjp2={reference_tile_headers:04x?}"
            );
    }

    let native_normalized = strip_com_segments(native_parts)
        .encode(&context.plan)
        .expect("re-encode native normalized codestream");
    let reference_normalized = strip_com_segments(reference_parts)
        .encode(&context.plan)
        .expect("re-encode openjp2 normalized codestream");

    if let Some((idx, lhs, rhs)) = first_byte_mismatch(&native_normalized, &reference_normalized) {
        let native_payload = payload_bytes(&strip_com_segments(
            CodestreamParts::parse_single_tile(&native_normalized)
                .expect("parse native normalized codestream"),
        ));
        let reference_payload = payload_bytes(&strip_com_segments(
            CodestreamParts::parse_single_tile(&reference_normalized)
                .expect("parse openjp2 normalized codestream"),
        ));
        let payload_mismatch = first_byte_mismatch(&native_payload, &reference_payload)
                .map(|(payload_idx, payload_lhs, payload_rhs)| {
                    format!(
                        "; payload mismatch at byte {payload_idx}: native=0x{payload_lhs:02x}, openjp2=0x{payload_rhs:02x}; native_payload_len={}, openjp2_payload_len={}",
                        native_payload.len(),
                        reference_payload.len()
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "; payload lengths native={}, openjp2={}",
                        native_payload.len(),
                        reference_payload.len()
                    )
                });
        panic!(
                "codestream mismatch at byte {idx}: native=0x{lhs:02x}, openjp2=0x{rhs:02x}; native_len={}, openjp2_len={}{}",
                native_normalized.len(),
                reference_normalized.len(),
                payload_mismatch
            );
    }

    assert_eq!(native_normalized.len(), reference_normalized.len());
}

fn assert_native_exact_roundtrip_via_openjp2(name: &str, image: &Image) {
    let context = gray_lossless_context(image);
    let native_bytes = NativeBackend
        .encode_codestream(&context)
        .expect("encode native codestream");
    assert_exact_gray_decode(name, &native_bytes, image);
}

fn assert_exact_gray_decode(name: &str, bytes: &[u8], image: &Image) {
    let decoded = decode_j2k_with_openjp2(bytes);
    let expected = image.components[0]
        .data
        .iter()
        .map(|&sample| sample as u8)
        .collect::<Vec<_>>();
    assert_eq!(
        decoded.len(),
        expected.len(),
        "{name}: decoded length {} did not match expected length {}",
        decoded.len(),
        expected.len()
    );
    if let Some((idx, (&actual, &expected))) = decoded
        .iter()
        .zip(expected.iter())
        .enumerate()
        .find(|(_, (actual, expected))| actual != expected)
    {
        panic!(
                "{name}: native grayscale roundtrip mismatch at sample {idx}: decoded={actual}, expected={expected}"
            );
    }
}

fn gray_lossless_context(image: &Image) -> EncodeContext<'_> {
    EncodeContext::new(
        image,
        &EncodeOptions {
            preset: Preset::DocumentHigh,
            format: OutputFormat::J2k,
        },
    )
    .expect("build context")
}

fn decode_j2k_with_openjp2(bytes: &[u8]) -> Vec<u8> {
    let mut stream = Stream::from_bytes(1 << 20, bytes.to_vec());
    let mut codec = Codec::new_decoder(CODEC_FORMAT::OPJ_CODEC_J2K).expect("create J2K decoder");
    let mut params = opj_dparameters_t::default();
    assert_eq!(codec.setup_decoder(&mut params), 1, "setup_decoder failed");
    let mut image = codec.read_header(&mut stream).expect("read_header");
    assert_eq!(
        codec.decode(&mut stream, &mut image),
        1,
        "OpenJPEG decode failed"
    );
    assert_eq!(
        codec.end_decompress(&mut stream),
        1,
        "OpenJPEG end_decompress failed"
    );
    let comps = image.comps().expect("decoded components");
    assert_eq!(comps.len(), 1, "expected grayscale decode");
    comps[0]
        .data()
        .expect("decoded grayscale data")
        .iter()
        .map(|&sample| sample as u8)
        .collect()
}

fn strip_com_segments(mut parts: CodestreamParts) -> CodestreamParts {
    parts.main_header_segments.retain(|segment| {
        segment.len() < 2 || u16::from_be_bytes([segment[0], segment[1]]) != 0xff64
    });
    parts
}
