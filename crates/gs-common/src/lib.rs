//! Text primitives shared by the console, the network protocol and the client:
//! the engine tokenizer (`COM_Parse` / `Cmd_TokenizeString`) and `\key\value` info strings.

pub mod bytestr;
pub mod conprint;
pub mod crc;
pub mod info;
pub mod net;
pub mod rng;
pub mod token;
