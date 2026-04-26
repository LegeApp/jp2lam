#[derive(Debug, Clone)]
pub(crate) struct PacketBytes {
    bytes: Vec<u8>,
}

impl PacketBytes {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PacketEncoding {
    Opaque(PacketBytes),
    HeaderBody {
        header: PacketBytes,
        body: PacketBytes,
    },
}

impl PacketEncoding {
    pub(crate) fn byte_len(&self) -> usize {
        match self {
            Self::Opaque(bytes) => bytes.len(),
            Self::HeaderBody { header, body } => header.len() + body.len(),
        }
    }

    pub(crate) fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            Self::Opaque(bytes) => out.extend_from_slice(bytes.as_slice()),
            Self::HeaderBody { header, body } => {
                out.extend_from_slice(header.as_slice());
                out.extend_from_slice(body.as_slice());
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Packet {
    encoding: PacketEncoding,
}

impl Packet {
    pub(crate) fn opaque(bytes: Vec<u8>) -> Self {
        Self {
            encoding: PacketEncoding::Opaque(PacketBytes::new(bytes)),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn header_body(header: Vec<u8>, body: Vec<u8>) -> Self {
        Self {
            encoding: PacketEncoding::HeaderBody {
                header: PacketBytes::new(header),
                body: PacketBytes::new(body),
            },
        }
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.encoding.byte_len()
    }

    pub(crate) fn write_to(&self, out: &mut Vec<u8>) {
        self.encoding.write_to(out);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PacketSequence {
    packets: Vec<Packet>,
}

impl PacketSequence {
    pub(crate) fn from_packets(packets: Vec<Packet>) -> Self {
        Self { packets }
    }

    pub(crate) fn from_opaque_bytes(bytes: Vec<u8>) -> Self {
        Self::from_packets(vec![Packet::opaque(bytes)])
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.packets.iter().map(Packet::byte_len).sum()
    }

    pub(crate) fn packet_count(&self) -> usize {
        self.packets.len()
    }

    pub(crate) fn write_to(&self, out: &mut Vec<u8>) {
        for packet in &self.packets {
            packet.write_to(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Packet, PacketBytes, PacketEncoding, PacketSequence};

    #[test]
    fn packet_sequence_preserves_opaque_bytes() {
        let seq = PacketSequence::from_packets(vec![
            Packet::opaque(vec![0xde, 0xad]),
            Packet::opaque(vec![0xbe, 0xef]),
        ]);
        let mut out = Vec::new();
        seq.write_to(&mut out);
        assert_eq!(seq.packet_count(), 2);
        assert_eq!(seq.byte_len(), 4);
        assert_eq!(out, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn header_body_packet_writes_in_order() {
        let packet = PacketEncoding::HeaderBody {
            header: PacketBytes::new(vec![0x01, 0x02]),
            body: PacketBytes::new(vec![0xa0, 0xb0, 0xc0]),
        };
        let mut out = Vec::new();
        packet.write_to(&mut out);
        assert_eq!(packet.byte_len(), 5);
        assert_eq!(out, vec![0x01, 0x02, 0xa0, 0xb0, 0xc0]);
    }
}
