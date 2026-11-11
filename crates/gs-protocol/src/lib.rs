//! GoldSrc protocol 48 message layer (docs/re/net-messages.md): bit buffers, the delta codec,
//! the stateful server message decoder with its entity frame ring, and client message builders.

pub mod bitbuf;
pub mod clc;
pub mod delta;
pub mod messages;
pub mod state;
pub mod structs;
pub mod tempent;

pub use bitbuf::{MsgReader, MsgWriter};
pub use messages::ServerMessage;
pub use state::ProtocolState;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("malformed server message: {0}")]
    Malformed(String),
}
