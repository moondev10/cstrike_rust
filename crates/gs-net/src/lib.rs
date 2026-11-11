//! GoldSrc protocol 48 transport layer (docs/re/net-transport.md).
//!
//! Message contents (svc/clc, delta encoding) live in `gs-protocol`; this crate moves bytes:
//! connectionless handshake packets, split datagram reassembly, the sequenced/reliable netchan
//! with its fragment streams, and packet munging.

pub mod munge;
pub mod netchan;
pub mod oob;
pub mod socket;
pub mod split;

pub use netchan::{Netchan, NetchanError, PacketHeader, Processed, ReceivedFile};
pub use socket::ClientSocket;
