use crate::encode::profile_enter;
use crate::error::Result;
use crate::t2::{PacketSequenceBuilder, TilePartPayload};

use super::t1::{NativeEncodedTier1Band, NativeEncodedTier1Layout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativePacket {
    pub resolution: u8,
    pub codeblock_count: usize,
    pub pass_count: usize,
    pub header: Vec<u8>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativePacketSequence {
    pub packets: Vec<NativePacket>,
}

// ---------------------------------------------------------------------------
// BIO — big-endian bit writer for packet headers (ISO 15444-1 §B.2)
// ---------------------------------------------------------------------------

struct PacketBio {
    buf: Vec<u8>,
    reg: u32,
    ct: u32,
}

impl PacketBio {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            reg: 0,
            ct: 8,
        }
    }

    fn byteout(&mut self) {
        self.reg = (self.reg << 8) & 0xffff;
        self.ct = if self.reg == 0xff00 { 7 } else { 8 };
        self.buf.push((self.reg >> 8) as u8);
    }

    fn putbit(&mut self, b: u32) {
        if self.ct == 0 {
            self.byteout();
        }
        self.ct -= 1;
        self.reg |= b << self.ct;
    }

    fn write_bits(&mut self, v: u32, n: u32) {
        for i in (0..n as i32).rev() {
            self.putbit((v >> i) & 1);
        }
    }

    fn flush(mut self) -> Vec<u8> {
        self.byteout();
        if self.ct == 7 {
            self.byteout();
        }
        self.buf
    }
}

// ---------------------------------------------------------------------------
// Tag tree — hierarchical min-value coder (ISO 15444-1 §B.10.2)
// ---------------------------------------------------------------------------

const TGT_NO_PARENT: u32 = u32::MAX;

#[derive(Clone)]
struct TgtNode {
    parent: u32,
    value: i32,
    low: i32,
    known: u8,
}

struct TagTree {
    nodes: Vec<TgtNode>,
    #[allow(dead_code)]
    numleafsh: usize,
    #[allow(dead_code)]
    numleafsv: usize,
}

impl TagTree {
    fn new(numleafsh: usize, numleafsv: usize) -> Self {
        let mut nplh = [0i32; 32];
        let mut nplv = [0i32; 32];
        let mut numlvls = 0usize;
        let mut numnodes = 0usize;
        nplh[0] = numleafsh as i32;
        nplv[0] = numleafsv as i32;
        loop {
            let n = (nplh[numlvls] * nplv[numlvls]) as usize;
            nplh[numlvls + 1] = (nplh[numlvls] + 1) / 2;
            nplv[numlvls + 1] = (nplv[numlvls] + 1) / 2;
            numnodes += n;
            numlvls += 1;
            if n <= 1 {
                break;
            }
        }
        let default_node = TgtNode {
            parent: TGT_NO_PARENT,
            value: 999,
            low: 0,
            known: 0,
        };
        let mut nodes = vec![default_node; numnodes];
        Self::link_parents(&mut nodes, numleafsh, numleafsv, numlvls, &nplh, &nplv);
        Self {
            nodes,
            numleafsh,
            numleafsv,
        }
    }

    fn link_parents(
        nodes: &mut [TgtNode],
        numleafsh: usize,
        numleafsv: usize,
        numlvls: usize,
        nplh: &[i32; 32],
        nplv: &[i32; 32],
    ) {
        let mut node_idx = 0usize;
        let mut l_parent_idx = numleafsh * numleafsv;
        let mut l_parent_idx0 = l_parent_idx;
        for i in 0..numlvls.saturating_sub(1) {
            let mut j = 0i32;
            while j < nplv[i] {
                let mut k = nplh[i];
                loop {
                    k -= 1;
                    if k < 0 {
                        break;
                    }
                    nodes[node_idx].parent = l_parent_idx as u32;
                    node_idx += 1;
                    k -= 1;
                    if k >= 0 {
                        nodes[node_idx].parent = l_parent_idx as u32;
                        node_idx += 1;
                    }
                    l_parent_idx += 1;
                }
                if j & 1 != 0 || j == nplv[i] - 1 {
                    l_parent_idx0 = l_parent_idx;
                } else {
                    l_parent_idx = l_parent_idx0;
                    l_parent_idx0 = l_parent_idx0 + nplh[i] as usize;
                }
                j += 1;
            }
        }
        if node_idx < nodes.len() {
            nodes[node_idx].parent = TGT_NO_PARENT;
        }
    }

    fn reset(&mut self) {
        for node in &mut self.nodes {
            node.value = 999;
            node.low = 0;
            node.known = 0;
        }
    }

    fn set_value(&mut self, leafno: usize, value: i32) {
        let mut idx = leafno;
        loop {
            let node = &mut self.nodes[idx];
            if node.value <= value {
                break;
            }
            node.value = value;
            if node.parent == TGT_NO_PARENT {
                break;
            }
            idx = node.parent as usize;
        }
    }

    fn encode(&mut self, bio: &mut PacketBio, leafno: usize, threshold: i32) {
        let mut stk = [0usize; 32];
        let mut depth = 0usize;
        let mut node_idx = leafno;
        while self.nodes[node_idx].parent != TGT_NO_PARENT {
            stk[depth] = node_idx;
            depth += 1;
            node_idx = self.nodes[node_idx].parent as usize;
        }
        let mut low = 0i32;
        loop {
            {
                let node = &mut self.nodes[node_idx];
                if low > node.low {
                    node.low = low;
                } else {
                    low = node.low;
                }
                while low < threshold {
                    if low >= node.value {
                        if node.known == 0 {
                            bio.putbit(1);
                            node.known = 1;
                        }
                        break;
                    } else {
                        bio.putbit(0);
                        low += 1;
                    }
                }
                node.low = low;
            }
            if depth == 0 {
                break;
            }
            depth -= 1;
            node_idx = stk[depth];
        }
    }
}

// ---------------------------------------------------------------------------
// Pass-count and comma-code helpers (ISO 15444-1 §B.10.6)
// ---------------------------------------------------------------------------

fn floor_log2(v: u32) -> u32 {
    if v == 0 {
        0
    } else {
        31 - v.leading_zeros()
    }
}

fn numpasses_codeword(numpasses: u32) -> (u32, u32) {
    // ISO/IEC 15444-1 Annex B.10.6, Table B-4.
    if numpasses == 1 {
        (0, 1)
    } else if numpasses == 2 {
        (0b10, 2)
    } else if numpasses <= 5 {
        (0xc | (numpasses - 3), 4)
    } else if numpasses <= 36 {
        (0x1e0 | (numpasses - 6), 9)
    } else if numpasses <= 164 {
        (0xff80 | (numpasses - 37), 16)
    } else {
        // Clamp at maximum supported pass count.
        (0xff80 | (164 - 37), 16)
    }
}

fn put_numpasses(bio: &mut PacketBio, numpasses: u32) {
    let (codeword, width) = numpasses_codeword(numpasses);
    bio.write_bits(codeword, width);
}

fn put_commacode(bio: &mut PacketBio, n: u32) {
    for _ in 0..n {
        bio.putbit(1);
    }
    bio.putbit(0);
}

fn codeblock_segments(passes: &[super::t1::NativeEncodedTier1Pass]) -> Vec<(u32, u32)> {
    let mut segments = Vec::new();
    let mut seg_len = 0u32;
    let mut seg_nump = 0u32;

    if passes.is_empty() {
        return segments;
    }

    let last = passes.len() - 1;
    for (passno, pass) in passes.iter().enumerate() {
        seg_nump += 1;
        seg_len += pass.length as u32;
        let boundary = pass.is_terminated || passno == last;
        if boundary {
            segments.push((seg_len, seg_nump));
            seg_len = 0;
            seg_nump = 0;
        }
    }

    segments
}

fn length_increment(numlenbits: u32, segments: &[(u32, u32)]) -> u32 {
    segments.iter().fold(0, |increment, &(seg_len, seg_nump)| {
        if seg_len == 0 {
            return increment;
        }
        let needed = floor_log2(seg_len) + 1;
        let have = numlenbits + floor_log2(seg_nump);
        increment.max(needed.saturating_sub(have))
    })
}

// ---------------------------------------------------------------------------
// Per-band codeblock count helpers
// ---------------------------------------------------------------------------

fn band_cb_dims(band: &NativeEncodedTier1Band) -> (usize, usize) {
    if band.blocks.is_empty() {
        return (0, 0);
    }
    let x0 = band.blocks.iter().map(|b| b.x0).min().unwrap();
    let y0 = band.blocks.iter().map(|b| b.y0).min().unwrap();
    let cbw = band.blocks[0].x1 - band.blocks[0].x0;
    let cbh = band.blocks[0].y1 - band.blocks[0].y0;
    if cbw == 0 || cbh == 0 {
        return (1, band.blocks.len());
    }
    let x1 = band.blocks.iter().map(|b| b.x1).max().unwrap();
    let y1 = band.blocks.iter().map(|b| b.y1).max().unwrap();
    let cx = (x1 - x0).div_ceil(cbw);
    let cy = (y1 - y0).div_ceil(cbh);
    (cx, cy)
}

// ---------------------------------------------------------------------------
// Real ISO 15444-1 packet header encoder
// ---------------------------------------------------------------------------

/// Build one full LRCP packet header for a single (layer, resolution,
/// component, precinct). `bands` are listed in codestream order (LL for the
/// lowest resolution; HL/LH/HH for detail resolutions).
fn build_packet_header(bands: &[&NativeEncodedTier1Band]) -> Vec<u8> {
    let mut bio = PacketBio::new();
    // Match OpenJPEG's current packet-header walk: emit the packet-present
    // bit and inclusion information for each non-empty band even when this
    // layer contributes no passes in that packet.
    bio.putbit(1);

    for band in bands {
        let n = band.blocks.len();
        if n == 0 {
            continue;
        }

        let (cx, cy) = band_cb_dims(band);
        let mut incltree = TagTree::new(cx.max(1), cy.max(1));
        let mut imsbtree = TagTree::new(cx.max(1), cy.max(1));
        incltree.reset();
        imsbtree.reset();

        for (cblkno, cblk) in band.blocks.iter().enumerate() {
            imsbtree.set_value(cblkno, cblk.zero_bitplanes as i32);
            if !cblk.passes.is_empty() {
                incltree.set_value(cblkno, 0);
            }
        }

        let mut numlenbits_per_block: Vec<u32> = vec![3; n];

        for (cblkno, cblk) in band.blocks.iter().enumerate() {
            incltree.encode(&mut bio, cblkno, 1);

            if cblk.passes.is_empty() {
                continue;
            }

            imsbtree.encode(&mut bio, cblkno, 999);

            let numpasses = cblk.passes.len() as u32;
            put_numpasses(&mut bio, numpasses);

            let numlenbits = numlenbits_per_block[cblkno];

            // ISO/IEC 15444-1 Annex B.10.7 applies one Lblock increment at the
            // start of the sequence, sized for the largest segment in the packet.
            let segments = codeblock_segments(&cblk.passes);
            let increment = length_increment(numlenbits, &segments);

            put_commacode(&mut bio, increment);
            numlenbits_per_block[cblkno] = numlenbits + increment;

            let numlenbits_final = numlenbits_per_block[cblkno];
            for (seg_len, seg_nump) in segments {
                let bits = numlenbits_final + floor_log2(seg_nump);
                bio.write_bits(seg_len, bits);
            }
        }
    }

    bio.flush()
}

// ---------------------------------------------------------------------------
// Packet body: concatenated pass bytes per codeblock
// ---------------------------------------------------------------------------

fn build_packet_body(band: &NativeEncodedTier1Band) -> Vec<u8> {
    let mut body = Vec::new();
    for cblk in &band.blocks {
        for pass in &cblk.passes {
            body.extend_from_slice(&pass.bytes);
        }
    }
    body
}

// ---------------------------------------------------------------------------
// Resolution-level packet grouping
// ---------------------------------------------------------------------------

pub(crate) fn build_packet_sequence(
    layout: &NativeEncodedTier1Layout,
) -> Result<NativePacketSequence> {
    build_packet_sequence_for_components(std::slice::from_ref(layout))
}

pub(crate) fn build_packet_sequence_for_components(
    layouts: &[NativeEncodedTier1Layout],
) -> Result<NativePacketSequence> {
    let _p = profile_enter("build_packet_sequence_for_components");
    let max_resolution = layouts
        .iter()
        .flat_map(|component| component.bands.iter().map(|band| band.resolution))
        .max()
        .unwrap_or(0);

    let mut packets = Vec::new();

    for resolution in 0..=max_resolution {
        // LRCP ordering with one tile and one precinct still emits packets per
        // component at each resolution (L=0 fixed): (R,C).
        for component in layouts {
            let bands: Vec<&NativeEncodedTier1Band> = component
                .bands
                .iter()
                .filter(|b| b.resolution == resolution)
                .collect();

            if bands.is_empty() {
                continue;
            }

            let header = build_packet_header(&bands);
            let mut body = Vec::new();
            let mut codeblock_count = 0usize;
            let mut pass_count = 0usize;
            for band in &bands {
                body.extend_from_slice(&build_packet_body(band));
                codeblock_count += band.blocks.len();
                pass_count += band
                    .blocks
                    .iter()
                    .map(|block| block.passes.len())
                    .sum::<usize>();
            }

            packets.push(NativePacket {
                resolution,
                codeblock_count,
                pass_count,
                header,
                body,
            });
        }
    }

    Ok(NativePacketSequence { packets })
}

pub(crate) fn build_tile_part_payload(
    layout: &NativeEncodedTier1Layout,
) -> Result<TilePartPayload> {
    build_tile_part_payload_for_components(std::slice::from_ref(layout))
}

pub(crate) fn build_tile_part_payload_for_components(
    layouts: &[NativeEncodedTier1Layout],
) -> Result<TilePartPayload> {
    let _p = profile_enter("t2::build_tile_part_payload_for_components");
    let packet_sequence = build_packet_sequence_for_components(layouts)?;
    let mut builder = PacketSequenceBuilder::new();

    for packet in packet_sequence.packets {
        builder = builder.push_header_body_packet(packet.header, packet.body);
    }

    Ok(builder.finish_payload())
}

// ---------------------------------------------------------------------------
// Retained for tests that use encode_placeholder_tier1
// ---------------------------------------------------------------------------
#[allow(dead_code)]
pub(crate) fn build_packet_sequence_from_layout(
    layout: &NativeEncodedTier1Layout,
) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    let packet_sequence = build_packet_sequence(layout)?;
    Ok(packet_sequence
        .packets
        .into_iter()
        .map(|packet| (packet.header, packet.body))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        build_tile_part_payload, codeblock_segments, floor_log2, length_increment,
        numpasses_codeword, put_commacode, put_numpasses, PacketBio, TagTree,
    };
    use crate::encode::backend::native::layout::build_component_layout;
    use crate::encode::backend::native::t1::{
        analyze_component_layout, encode_placeholder_tier1, NativeEncodedTier1Pass,
        NativeTier1PassCodingMode, NativeTier1PassKind, NativeTier1PassTermination,
    };
    use crate::encode::backend::native::NativeComponentCoefficients;
    use crate::plan::CodeBlockSize;

    fn mock_pass(pass_index: u16, length: usize, is_terminated: bool) -> NativeEncodedTier1Pass {
        NativeEncodedTier1Pass {
            kind: NativeTier1PassKind::Cleanup,
            bitplane: 0,
            pass_index,
            coding_mode: NativeTier1PassCodingMode::Mq,
            termination: NativeTier1PassTermination::TermAll,
            segmark: false,
            is_terminated,
            newly_significant: 0,
            significant_before: 0,
            length,
            cumulative_length: 0,
            distortion_hint: 0,
            bytes: Vec::new(),
        }
    }

    #[test]
    fn bio_putbit_roundtrip_indicator_bit() {
        let mut bio = PacketBio::new();
        bio.putbit(1);
        let bytes = bio.flush();
        assert!(!bytes.is_empty());
        assert_eq!(bytes[0] & 0x80, 0x80, "indicator bit should be MSB");
    }

    #[test]
    fn tagtree_single_leaf_encodes_inclusion() {
        let mut tree = TagTree::new(1, 1);
        tree.reset();
        tree.set_value(0, 0);
        let mut bio = PacketBio::new();
        tree.encode(&mut bio, 0, 1);
        let bytes = bio.flush();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn floor_log2_values() {
        assert_eq!(floor_log2(1), 0);
        assert_eq!(floor_log2(2), 1);
        assert_eq!(floor_log2(3), 1);
        assert_eq!(floor_log2(4), 2);
        assert_eq!(floor_log2(127), 6);
        assert_eq!(floor_log2(128), 7);
    }

    #[test]
    fn numpasses_writes_1_as_0_bit() {
        let mut bio = PacketBio::new();
        put_numpasses(&mut bio, 1);
        let bytes = bio.flush();
        assert_eq!(bytes[0] & 0x80, 0, "numpasses=1 should write a 0 bit first");
    }

    #[test]
    fn numpasses_writes_2_as_10() {
        let mut bio = PacketBio::new();
        put_numpasses(&mut bio, 2);
        let bytes = bio.flush();
        assert_eq!(
            bytes[0] >> 6,
            0b10,
            "numpasses=2 should write 10 in top 2 bits"
        );
    }

    #[test]
    fn b106_numpasses_boundary_values_follow_table_b4() {
        let cases = [
            (3, 4, 0b1100u16),
            (5, 4, 0b1110u16),
            (6, 9, 0b1111_00000u16),
            (36, 9, 0b1111_11110u16),
            (37, 16, 0b11111111_10000000u16),
            (164, 16, 0b11111111_11111111u16),
        ];

        for (numpasses, width, expected_prefix) in cases {
            let (actual, actual_width) = numpasses_codeword(numpasses);
            assert_eq!(
                (actual, actual_width),
                (expected_prefix as u32, width),
                "B.10.6 codeword mismatch for numpasses={numpasses}"
            );
        }
    }

    #[test]
    fn b107_commacode_zero_writes_single_0() {
        let mut bio = PacketBio::new();
        put_commacode(&mut bio, 0);
        let bytes = bio.flush();
        assert_eq!(
            bytes[0] & 0x80,
            0,
            "B.10.7 commacode(0) should write a 0 bit"
        );
    }

    #[test]
    fn b1071_single_segment_uses_initial_lblock_three() {
        let segments = codeblock_segments(&[mock_pass(0, 6, true)]);
        assert_eq!(segments, vec![(6, 1)]);
        assert_eq!(length_increment(3, &segments), 0);
    }

    #[test]
    fn b1072_multiple_segments_match_standard_note_partition() {
        // ISO/IEC 15444-1 Annex B.10.7.2 note:
        // pass lengths {6, 31, 44, 134, 192} with terminated-pass set
        // T = {0, 2, 3, 4} yields signaled segment lengths {6, 75, 134, 192}
        // and added-pass counts {1, 2, 1, 1}.
        let passes = vec![
            mock_pass(0, 6, true),
            mock_pass(1, 31, false),
            mock_pass(2, 44, true),
            mock_pass(3, 134, true),
            mock_pass(4, 192, true),
        ];
        let segments = codeblock_segments(&passes);
        assert_eq!(segments, vec![(6, 1), (75, 2), (134, 1), (192, 1)]);
        assert_eq!(length_increment(3, &segments), 5);
    }

    #[test]
    fn tile_part_payload_has_correct_packet_count() {
        let coefficients = NativeComponentCoefficients {
            width: 4,
            height: 4,
            levels: 2,
            data: vec![-38, 36, 0, 16, 144, 0, 0, 16, 0, 0, 0, 0, 64, 64, 0, 0],
        };
        let layout = build_component_layout(
            &coefficients,
            CodeBlockSize {
                width: 64,
                height: 64,
            },
        )
        .expect("build layout");
        let analyzed = analyze_component_layout(&layout);
        let encoded = encode_placeholder_tier1(&analyzed);
        let payload = build_tile_part_payload(&encoded).expect("build tile part payload");
        assert_eq!(payload.packet_count(), 3);
    }

    #[test]
    fn tile_part_payload_headers_start_with_indicator_bit() {
        let coefficients = NativeComponentCoefficients {
            width: 4,
            height: 4,
            levels: 2,
            data: vec![-38, 36, 0, 16, 144, 0, 0, 16, 0, 0, 0, 0, 64, 64, 0, 0],
        };
        let layout = build_component_layout(
            &coefficients,
            CodeBlockSize {
                width: 64,
                height: 64,
            },
        )
        .expect("build layout");
        let analyzed = analyze_component_layout(&layout);
        let encoded = encode_placeholder_tier1(&analyzed);
        let payload = build_tile_part_payload(&encoded).expect("build tile part payload");
        let mut out = Vec::new();
        payload.write_to(&mut out);
        assert!(!out.is_empty());
    }
}
