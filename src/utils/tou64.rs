// src/utils/tou64.rs

pub fn peer_id_to_u64(peer_id: &libp2p::PeerId) -> u64 {
    let bytes = peer_id.to_bytes();
    // Tomar los últimos 8 bytes del digest
    let start = bytes.len().saturating_sub(8);
    let slice = &bytes[start..];
    let mut arr = [0u8; 8];
    arr[..slice.len()].copy_from_slice(slice);
    u64::from_be_bytes(arr)
}

pub fn string_to_u64_rust(text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut s = DefaultHasher::new();
    text.hash(&mut s);
    s.finish()
}
