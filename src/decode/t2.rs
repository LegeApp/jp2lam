//! Tier-2 packet-header decoding primitives.
//!
//! This is the inverse side of ISO/IEC 15444-1 Annex B.10. It deliberately
//! starts with independently testable syntax helpers before wiring the full
//! LRCP packet walk.

use crate::error::{Jp2LamError, Result};
use crate::j2k::decode_markers::{CodestreamHeader, ProgressionOrder};
use crate::plan::BandOrientation;

const TGT_NO_PARENT: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedTilePackets<'a> {
    pub(crate) packets: Vec<DecodedPacket>,
    pub(crate) codeblocks: Vec<DecodedCodeBlock<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedPacket {
    pub(crate) layer: u16,
    pub(crate) resolution: u8,
    pub(crate) component: usize,
    pub(crate) header_len: usize,
    pub(crate) body_len: usize,
    pub(crate) contribution_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedCodeBlock<'a> {
    pub(crate) component: usize,
    pub(crate) band_index: usize,
    pub(crate) block_index: usize,
    pub(crate) resolution: u8,
    pub(crate) band: BandOrientation,
    pub(crate) x0: u32,
    pub(crate) y0: u32,
    pub(crate) x1: u32,
    pub(crate) y1: u32,
    pub(crate) zero_bitplanes: u32,
    pub(crate) passes: u32,
    pub(crate) data: &'a [u8],
}

#[derive(Debug, Clone)]
struct BandState {
    component: usize,
    resolution: u8,
    band: BandOrientation,
    blocks: Vec<BlockState>,
    inclusion: TagTreeReader,
    zero_bitplanes: TagTreeReader,
}

#[derive(Debug, Clone)]
struct BlockState {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    included: bool,
    numlenbits: u32,
    zero_bitplanes: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct Contribution {
    band_index: usize,
    block_index: usize,
    zero_bitplanes: u32,
    passes: u32,
    length: usize,
}

pub(crate) fn parse_tile_part_payload<'a>(
    header: &CodestreamHeader,
    payload: &'a [u8],
) -> Result<DecodedTilePackets<'a>> {
    validate_packet_scope(header)?;
    let mut bands = build_band_states(header)?;
    let mut pos = 0usize;
    let mut packets = Vec::new();
    let mut codeblocks = Vec::new();

    for layer in 0..header.cod.layers {
        for resolution in 0..=header.cod.decomposition_levels {
            for component in 0..header.siz.components.len() {
                let packet_start = pos;
                let mut bio = PacketBioReader::new(
                    payload
                        .get(packet_start..)
                        .ok_or_else(|| invalid("packet offset past tile payload"))?,
                );
                let packet_present = bio.read_bit()? != 0;
                let mut contributions = Vec::new();

                if packet_present {
                    for band_index in bands_for_component_resolution(&bands, component, resolution) {
                        read_band_contributions(
                            &mut bio,
                            layer,
                            band_index,
                            &mut bands,
                            &mut contributions,
                        )?;
                    }
                }

                let header_len = bio.bytes_consumed();
                pos = pos
                    .checked_add(header_len)
                    .ok_or_else(|| invalid("packet header offset overflow"))?;
                let body_len = contributions.iter().map(|c| c.length).sum::<usize>();
                let body_end = pos
                    .checked_add(body_len)
                    .ok_or_else(|| invalid("packet body offset overflow"))?;
                let body = payload
                    .get(pos..body_end)
                    .ok_or_else(|| invalid("packet body extends past tile payload"))?;
                let mut body_pos = 0usize;

                let contribution_count = contributions.len();
                for contribution in contributions {
                    let end = body_pos + contribution.length;
                    let data = &body[body_pos..end];
                    body_pos = end;
                    let band_state = &bands[contribution.band_index];
                    let block = &band_state.blocks[contribution.block_index];
                    codeblocks.push(DecodedCodeBlock {
                        component: band_state.component,
                        band_index: contribution.band_index,
                        block_index: contribution.block_index,
                        resolution,
                        band: band_state.band,
                        x0: block.x0,
                        y0: block.y0,
                        x1: block.x1,
                        y1: block.y1,
                        zero_bitplanes: contribution.zero_bitplanes,
                        passes: contribution.passes,
                        data,
                    });
                }
                pos = body_end;
                packets.push(DecodedPacket {
                    layer,
                    resolution,
                    component,
                    header_len,
                    body_len,
                    contribution_count,
                });
            }
        }
    }

    if pos != payload.len() {
        return Err(invalid(format!(
            "tile payload has {} trailing bytes after packet parse",
            payload.len() - pos
        )));
    }

    Ok(DecodedTilePackets {
        packets,
        codeblocks,
    })
}

fn validate_packet_scope(header: &CodestreamHeader) -> Result<()> {
    if header.cod.progression_order != ProgressionOrder::Lrcp {
        return Err(invalid("only LRCP packet decoding is implemented"));
    }
    if header.siz.components.len() != 1 && header.siz.components.len() != 3 {
        return Err(invalid("only one- and three-component packet decoding is implemented"));
    }
    if header.cod.uses_precincts || header.cod.sop_markers || header.cod.eph_markers {
        return Err(invalid("precinct, SOP, and EPH packet syntax is unsupported"));
    }
    if header.cod.code_block_style.bypass
        || header.cod.code_block_style.reset_contexts
        || header.cod.code_block_style.terminate_each_pass
        || header.cod.code_block_style.vertical_causal
        || header.cod.code_block_style.predictable_termination
        || header.cod.code_block_style.segmentation_symbols
    {
        return Err(invalid(
            "non-default code-block style packet segmentation is unsupported",
        ));
    }
    Ok(())
}

fn read_band_contributions(
    bio: &mut PacketBioReader<'_>,
    layer: u16,
    band_index: usize,
    bands: &mut [BandState],
    contributions: &mut Vec<Contribution>,
) -> Result<()> {
    for block_index in 0..bands[band_index].blocks.len() {
        let was_included = bands[band_index].blocks[block_index].included;
        let included = if was_included {
            bio.read_bit()? != 0
        } else {
            bands[band_index].inclusion.decode(
                bio,
                block_index,
                i32::from(layer) + 1,
            )?
        };

        if !included {
            continue;
        }

        let zero_bitplanes = if let Some(value) = bands[band_index].blocks[block_index].zero_bitplanes
        {
            value
        } else {
            let tree = &mut bands[band_index].zero_bitplanes;
            tree.decode(bio, block_index, 999)?;
            let value = tree
                .value(block_index)
                .ok_or_else(|| invalid("zero-bitplane tag tree did not terminate"))?;
            let value = u32::try_from(value)
                .map_err(|_| invalid("negative zero-bitplane tag-tree value"))?;
            bands[band_index].blocks[block_index].zero_bitplanes = Some(value);
            value
        };

        let passes = read_numpasses(bio)?;
        let increment = read_commacode(bio)?;
        bands[band_index].blocks[block_index].numlenbits += increment;
        let len_bits =
            bands[band_index].blocks[block_index].numlenbits + floor_log2(passes);
        let length = bio.read_bits(len_bits)?;
        let length = usize::try_from(length)
            .map_err(|_| invalid("code-block contribution length exceeds usize"))?;
        if length == 0 {
            return Err(invalid(
                "non-empty code-block contribution has zero byte length",
            ));
        }
        bands[band_index].blocks[block_index].included = true;
        contributions.push(Contribution {
            band_index,
            block_index,
            zero_bitplanes,
            passes,
            length,
        });
    }
    Ok(())
}

fn bands_for_component_resolution(
    bands: &[BandState],
    component: usize,
    resolution: u8,
) -> Vec<usize> {
    bands
        .iter()
        .enumerate()
        .filter_map(|(index, band)| {
            (band.component == component && band.resolution == resolution).then_some(index)
        })
        .collect()
}

fn build_band_states(header: &CodestreamHeader) -> Result<Vec<BandState>> {
    let width = header.siz.width;
    let height = header.siz.height;
    let levels = header.cod.decomposition_levels;
    let cbw = header.cod.code_block_width;
    let cbh = header.cod.code_block_height;
    let resolutions = resolution_ladder(width, height, levels);
    let component_count = header.siz.components.len();
    let mut bands = Vec::with_capacity(component_count * (1 + usize::from(levels) * 3));

    for component in 0..component_count {
        let ll = resolutions[0];
        push_band(&mut bands, component, 0, BandOrientation::Ll, 0, 0, ll.0, ll.1, cbw, cbh);

        for index in 0..usize::from(levels) {
            let resolution = (index + 1) as u8;
            let low = resolutions[index];
            let full = resolutions[index + 1];
            push_band(
                &mut bands,
                component,
                resolution,
                BandOrientation::Hl,
                low.0,
                0,
                full.0,
                low.1,
                cbw,
                cbh,
            );
            push_band(
                &mut bands,
                component,
                resolution,
                BandOrientation::Lh,
                0,
                low.1,
                low.0,
                full.1,
                cbw,
                cbh,
            );
            push_band(
                &mut bands,
                component,
                resolution,
                BandOrientation::Hh,
                low.0,
                low.1,
                full.0,
                full.1,
                cbw,
                cbh,
            );
        }
    }

    Ok(bands)
}

#[allow(clippy::too_many_arguments)]
fn push_band(
    bands: &mut Vec<BandState>,
    component: usize,
    resolution: u8,
    band: BandOrientation,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    cbw: u32,
    cbh: u32,
) {
    let blocks = split_blocks(x0, y0, x1, y1, cbw, cbh);
    let (leaf_w, leaf_h) = block_grid_dims(&blocks);
    bands.push(BandState {
        component,
        resolution,
        band,
        blocks,
        inclusion: TagTreeReader::new(leaf_w.max(1), leaf_h.max(1)),
        zero_bitplanes: TagTreeReader::new(leaf_w.max(1), leaf_h.max(1)),
    });
}

fn split_blocks(x0: u32, y0: u32, x1: u32, y1: u32, cbw: u32, cbh: u32) -> Vec<BlockState> {
    let mut blocks = Vec::new();
    let mut by = y0;
    while by < y1 {
        let block_y1 = (by + cbh).min(y1);
        let mut bx = x0;
        while bx < x1 {
            let block_x1 = (bx + cbw).min(x1);
            blocks.push(BlockState {
                x0: bx,
                y0: by,
                x1: block_x1,
                y1: block_y1,
                included: false,
                numlenbits: 3,
                zero_bitplanes: None,
            });
            bx = block_x1;
        }
        by = block_y1;
    }
    blocks
}

fn block_grid_dims(blocks: &[BlockState]) -> (usize, usize) {
    if blocks.is_empty() {
        return (0, 0);
    }
    let x0 = blocks.iter().map(|b| b.x0).min().unwrap();
    let y0 = blocks.iter().map(|b| b.y0).min().unwrap();
    let cbw = blocks[0].x1 - blocks[0].x0;
    let cbh = blocks[0].y1 - blocks[0].y0;
    if cbw == 0 || cbh == 0 {
        return (1, blocks.len());
    }
    let x1 = blocks.iter().map(|b| b.x1).max().unwrap();
    let y1 = blocks.iter().map(|b| b.y1).max().unwrap();
    (((x1 - x0).div_ceil(cbw)) as usize, ((y1 - y0).div_ceil(cbh)) as usize)
}

fn resolution_ladder(width: u32, height: u32, levels: u8) -> Vec<(u32, u32)> {
    let mut resolutions = Vec::with_capacity(usize::from(levels) + 1);
    let mut w = width;
    let mut h = height;
    resolutions.push((w, h));
    for _ in 0..levels {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        resolutions.push((w, h));
    }
    resolutions.reverse();
    resolutions
}

fn floor_log2(v: u32) -> u32 {
    if v == 0 {
        0
    } else {
        31 - v.leading_zeros()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketBioReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    reg: u8,
    ct: u8,
    previous_was_ff: bool,
}

impl<'a> PacketBioReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            reg: 0,
            ct: 0,
            previous_was_ff: false,
        }
    }

    pub(crate) fn read_bit(&mut self) -> Result<u32> {
        if self.ct == 0 {
            self.bytein()?;
        }
        self.ct -= 1;
        Ok(u32::from((self.reg >> self.ct) & 1))
    }

    pub(crate) fn read_bits(&mut self, count: u32) -> Result<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | self.read_bit()?;
        }
        Ok(value)
    }

    pub(crate) fn bytes_consumed(&self) -> usize {
        self.pos
    }

    fn bytein(&mut self) -> Result<()> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| invalid("packet header ended before requested bit"))?;
        self.pos += 1;
        self.reg = byte;
        self.ct = if self.previous_was_ff { 7 } else { 8 };
        self.previous_was_ff = byte == 0xff;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TagTreeReader {
    nodes: Vec<TgtNode>,
}

#[derive(Debug, Clone)]
struct TgtNode {
    parent: u32,
    value: i32,
    low: i32,
    known: bool,
}

impl TagTreeReader {
    pub(crate) fn new(numleafsh: usize, numleafsv: usize) -> Self {
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

        let mut nodes = vec![
            TgtNode {
                parent: TGT_NO_PARENT,
                value: i32::MAX,
                low: 0,
                known: false,
            };
            numnodes
        ];
        link_parents(&mut nodes, numleafsh, numleafsv, numlvls, &nplh, &nplv);
        Self { nodes }
    }

    /// Decode whether `leafno` is included below `threshold`.
    ///
    /// Returns `true` when the decoded tag-tree value is less than the supplied
    /// threshold. The decoded value remains stored in the tree so later packets
    /// can continue from the Annex B.10.2 `low` state.
    pub(crate) fn decode(
        &mut self,
        bio: &mut PacketBioReader<'_>,
        leafno: usize,
        threshold: i32,
    ) -> Result<bool> {
        if leafno >= self.nodes.len() {
            return Err(invalid("tag-tree leaf index out of range"));
        }

        let mut stack = [0usize; 32];
        let mut depth = 0usize;
        let mut node_idx = leafno;
        while self.nodes[node_idx].parent != TGT_NO_PARENT {
            stack[depth] = node_idx;
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
                while low < threshold && !node.known {
                    if bio.read_bit()? == 1 {
                        node.value = low;
                        node.known = true;
                    } else {
                        low += 1;
                    }
                }
                if node.known && node.value > low {
                    low = node.value;
                }
                node.low = low;
            }

            if depth == 0 {
                break;
            }
            depth -= 1;
            node_idx = stack[depth];
        }

        Ok(self.nodes[leafno].known && self.nodes[leafno].value < threshold)
    }

    pub(crate) fn value(&self, leafno: usize) -> Option<i32> {
        self.nodes
            .get(leafno)
            .and_then(|node| node.known.then_some(node.value))
    }
}

pub(crate) fn read_commacode(bio: &mut PacketBioReader<'_>) -> Result<u32> {
    let mut count = 0u32;
    while bio.read_bit()? == 1 {
        count = count
            .checked_add(1)
            .ok_or_else(|| invalid("comma-code length overflow"))?;
    }
    Ok(count)
}

pub(crate) fn read_numpasses(bio: &mut PacketBioReader<'_>) -> Result<u32> {
    // ISO/IEC 15444-1 Annex B.10.6, Table B-4.
    if bio.read_bit()? == 0 {
        return Ok(1);
    }
    if bio.read_bit()? == 0 {
        return Ok(2);
    }
    let next = bio.read_bits(2)?;
    if next != 0b11 {
        return Ok(3 + next);
    }
    let next = bio.read_bits(5)?;
    if next != 0b1_1111 {
        return Ok(6 + next);
    }
    Ok(37 + bio.read_bits(7)?)
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
                l_parent_idx0 += nplh[i] as usize;
            }
            j += 1;
        }
    }
    if node_idx < nodes.len() {
        nodes[node_idx].parent = TGT_NO_PARENT;
    }
}

fn invalid(message: impl Into<String>) -> Jp2LamError {
    Jp2LamError::DecodeFailed(message.into())
}

#[cfg(test)]
mod tests {
    use super::{parse_tile_part_payload, read_commacode, read_numpasses, PacketBioReader, TagTreeReader};
    use crate::decode::{
        CodeBlockStyle, CodSegment, CodestreamHeader, ComponentSiz, ProgressionOrder, QcdSegment,
        QuantizationStep, QuantizationStyle, SizSegment, WaveletTransform,
    };

    #[test]
    fn packet_bio_reads_msb_first() {
        let mut bio = PacketBioReader::new(&[0b1010_0000]);
        assert_eq!(bio.read_bit().unwrap(), 1);
        assert_eq!(bio.read_bit().unwrap(), 0);
        assert_eq!(bio.read_bit().unwrap(), 1);
        assert_eq!(bio.read_bit().unwrap(), 0);
        assert_eq!(bio.bytes_consumed(), 1);
    }

    #[test]
    fn packet_bio_skips_ff_stuffed_bit() {
        let mut bio = PacketBioReader::new(&[0xff, 0b1010_1010]);
        for _ in 0..8 {
            assert_eq!(bio.read_bit().unwrap(), 1);
        }
        // The MSB after an 0xff byte is a stuffed bit. The next data bits are
        // read from bit 6 downwards.
        assert_eq!(bio.read_bit().unwrap(), 0);
        assert_eq!(bio.read_bit().unwrap(), 1);
    }

    #[test]
    fn b107_commacode_reads_unary_one_bits_then_zero() {
        let mut bio = PacketBioReader::new(&[0b1110_0000]);
        assert_eq!(read_commacode(&mut bio).unwrap(), 3);
    }

    #[test]
    fn b106_numpasses_reads_table_b4_boundaries() {
        let cases = [
            (&[0b0000_0000][..], 1),
            (&[0b1000_0000][..], 2),
            (&[0b1100_0000][..], 3),
            (&[0b1110_0000][..], 5),
            (&[0b1111_0000, 0][..], 6),
            (&[0b1111_1111, 0b0100_0000, 0][..], 37),
            (&[0b1111_1111, 0b0111_1111, 0b1000_0000][..], 164),
        ];
        for (bytes, expected) in cases {
            let mut bio = PacketBioReader::new(bytes);
            assert_eq!(read_numpasses(&mut bio).unwrap(), expected);
        }
    }

    #[test]
    fn b102_tagtree_single_leaf_decodes_value_zero() {
        let mut tree = TagTreeReader::new(1, 1);
        let mut bio = PacketBioReader::new(&[0b1000_0000]);
        assert!(tree.decode(&mut bio, 0, 1).unwrap());
        assert_eq!(tree.value(0), Some(0));
    }

    #[test]
    fn b102_tagtree_single_leaf_decodes_later_threshold() {
        let mut tree = TagTreeReader::new(1, 1);
        let mut bio = PacketBioReader::new(&[0b0010_0000]);
        assert!(!tree.decode(&mut bio, 0, 1).unwrap());
        assert!(!tree.decode(&mut bio, 0, 2).unwrap());
        assert!(tree.decode(&mut bio, 0, 3).unwrap());
        assert_eq!(tree.value(0), Some(2));
    }

    #[test]
    fn b107_non_empty_contribution_with_zero_length_fails_fast() {
        let header = tiny_header();
        let payload = [0b1110_0000];

        let err = parse_tile_part_payload(&header, &payload)
            .expect_err("included code-block with zero length should fail")
            .to_string();

        assert!(err.contains("zero byte length"), "{err}");
    }

    fn tiny_header() -> CodestreamHeader {
        CodestreamHeader {
            siz: SizSegment {
                rsiz: 0,
                width: 1,
                height: 1,
                x_origin: 0,
                y_origin: 0,
                tile_width: 1,
                tile_height: 1,
                tile_x_origin: 0,
                tile_y_origin: 0,
                components: vec![ComponentSiz {
                    precision: 8,
                    signed: false,
                    dx: 1,
                    dy: 1,
                }],
            },
            cod: CodSegment {
                progression_order: ProgressionOrder::Lrcp,
                layers: 1,
                use_mct: false,
                decomposition_levels: 0,
                code_block_width: 64,
                code_block_height: 64,
                code_block_style: CodeBlockStyle::default(),
                transform: WaveletTransform::Irreversible97,
                uses_precincts: false,
                sop_markers: false,
                eph_markers: false,
                precinct_sizes: Vec::new(),
            },
            qcd: QcdSegment {
                style: QuantizationStyle::ScalarExpounded,
                guard_bits: 1,
                steps: vec![QuantizationStep {
                    exponent: 8,
                    mantissa: 0,
                }],
            },
            comment_count: 0,
        }
    }
}
