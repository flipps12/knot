# Knot

### Description

Knot is a peer-to-peer overlay network that enables direct communication between devices over the Internet without relying on central servers. It uses cryptographic identities to connect nodes and establish secure connections even behind NAT. Its goal is to provide a foundation for decentralized applications such as messaging, file sharing, and data synchronization.

### Solution

Use a daemon that recognizes and maintains open connections through hole punching and the QUIC and UDP protocols.

### Technologies

- Rust
    - Libp2p
    - Tokio
- Unix & Tcp sockets
- QUIC & UDP
