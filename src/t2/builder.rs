use super::packet::{Packet, PacketSequence};
use super::payload::TilePartPayload;

#[derive(Debug, Default)]
pub(crate) struct PacketSequenceBuilder {
    packets: Vec<Packet>,
}

#[allow(dead_code)]
impl PacketSequenceBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push_opaque_packet(mut self, bytes: Vec<u8>) -> Self {
        self.packets.push(Packet::opaque(bytes));
        self
    }

    pub(crate) fn push_header_body_packet(mut self, header: Vec<u8>, body: Vec<u8>) -> Self {
        self.packets.push(Packet::header_body(header, body));
        self
    }

    pub(crate) fn packet_count(&self) -> usize {
        self.packets.len()
    }

    pub(crate) fn finish(self) -> PacketSequence {
        PacketSequence::from_packets(self.packets)
    }

    pub(crate) fn finish_payload(self) -> TilePartPayload {
        TilePartPayload::from_packet_sequence(self.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::PacketSequenceBuilder;

    #[test]
    fn builder_constructs_mixed_packet_sequence() {
        let payload = PacketSequenceBuilder::new()
            .push_header_body_packet(vec![0x01], vec![0xa0, 0xa1])
            .push_opaque_packet(vec![0xbe, 0xef])
            .finish_payload();
        let mut out = Vec::new();
        payload.write_to(&mut out);
        assert_eq!(payload.packet_count(), 2);
        assert_eq!(payload.byte_len(), 5);
        assert_eq!(out, vec![0x01, 0xa0, 0xa1, 0xbe, 0xef]);
    }
}
