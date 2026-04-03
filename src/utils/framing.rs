// src/utils/framing.rs

use bytes::{Bytes, BytesMut, BufMut, Buf};

#[derive(Debug)]
pub struct BinaryFrame {
    pub version: u8,
    pub target: u8,
    pub peer_id: u64,
    pub app_id: u64,
    pub payload: Bytes,
}

impl BinaryFrame {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(8 + self.payload.len());
        buf.put_u8(self.version);
        buf.put_u8(self.target);
        buf.put_u64(self.peer_id);
        buf.put_u64(self.app_id);
        buf.put_u32(self.payload.len() as u32);
        buf.put_u16(0); // Reserved
        buf.put(&self.payload[..]);
        buf.freeze()
    }

    pub fn decode(mut buf: BytesMut) -> Self {
        let version = buf.get_u8();
        let target = buf.get_u8();
        let peer_id = buf.get_u64();
        let app_id = buf.get_u64();
        let _len = buf.get_u32();
        let _reserved = buf.get_u16();
        let payload = buf.split_to(buf.len()).freeze();

        BinaryFrame { version, target, peer_id, app_id, payload }
    }

    pub fn from_raw(header: &[u8; 24], payload: Bytes) -> Self {
        Self {
            version: header[0],
            target: header[1],
            peer_id: u64::from_be_bytes(header[2..10].try_into().unwrap()),
            app_id: u64::from_be_bytes(header[10..18].try_into().unwrap()),
            payload,
        }
    }
}