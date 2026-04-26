use super::packet::{Packet, PacketSequence};

#[derive(Debug, Clone)]
pub(crate) enum TilePartPayload {
    PacketSequence(PacketSequence),
}

impl TilePartPayload {
    pub(crate) fn from_raw_bytes(bytes: Vec<u8>) -> Self {
        Self::PacketSequence(PacketSequence::from_opaque_bytes(bytes))
    }

    pub(crate) fn from_packet_sequence(sequence: PacketSequence) -> Self {
        Self::PacketSequence(sequence)
    }

    #[allow(dead_code)]
    pub(crate) fn from_packets(packets: Vec<Packet>) -> Self {
        Self::from_packet_sequence(PacketSequence::from_packets(packets))
    }

    pub(crate) fn byte_len(&self) -> usize {
        match self {
            Self::PacketSequence(sequence) => sequence.byte_len(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn packet_count(&self) -> usize {
        match self {
            Self::PacketSequence(sequence) => sequence.packet_count(),
        }
    }

    pub(crate) fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            Self::PacketSequence(sequence) => sequence.write_to(out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TilePartPayload;
    use crate::t2::Packet;

    #[test]
    fn payload_from_packets_preserves_header_body_order() {
        let payload = TilePartPayload::from_packets(vec![
            Packet::header_body(vec![0x01, 0x02], vec![0xa0]),
            Packet::opaque(vec![0xbb, 0xcc]),
        ]);
        let mut out = Vec::new();
        payload.write_to(&mut out);
        assert_eq!(payload.packet_count(), 2);
        assert_eq!(payload.byte_len(), 5);
        assert_eq!(out, vec![0x01, 0x02, 0xa0, 0xbb, 0xcc]);
    }
}
