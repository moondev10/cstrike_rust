//! Client→server message builders (docs/re/net-messages.md §6–7).

use gs_common::crc::{CRC32_TABLE, Crc32};
use gs_net::munge::{MungeTable, munge};

use crate::bitbuf::MsgWriter;
use crate::delta::Delta;
use crate::structs::UserCmd;

pub mod op {
    pub const NOP: u8 = 1;
    pub const MOVE: u8 = 2;
    pub const STRINGCMD: u8 = 3;
    pub const DELTA: u8 = 4;
    pub const RESOURCELIST: u8 = 5;
    pub const FILECONSISTENCY: u8 = 7;
    pub const VOICEDATA: u8 = 8;
    pub const CVARVALUE: u8 = 10;
    pub const CVARVALUE2: u8 = 11;
}

/// The server rejects more than this many commands in one `clc_move`.
pub const MAX_MOVE_COMMANDS: usize = 62;

pub fn stringcmd(w: &mut MsgWriter, text: &str) {
    w.write_u8(op::STRINGCMD);
    w.write_string(text);
}

/// `clc_voicedata` (`CL_AddVoiceToDatagram`): `uint16 length` + the compressed voice bytes.
pub fn voicedata(w: &mut MsgWriter, data: &[u8]) {
    w.write_u8(op::VOICEDATA);
    w.write_i16(data.len() as u16 as i16);
    w.write_bytes(data);
}

pub fn delta(w: &mut MsgWriter, valid_sequence: u32) {
    w.write_u8(op::DELTA);
    w.write_u8(valid_sequence as u8);
}

pub fn cvar_value(w: &mut MsgWriter, value: &str) {
    w.write_u8(op::CVARVALUE);
    w.write_string(value);
}

pub fn cvar_value2(w: &mut MsgWriter, request_id: i32, name: &str, value: &str) {
    w.write_u8(op::CVARVALUE2);
    w.write_i32(request_id);
    w.write_string(name);
    w.write_string(value);
}

/// `clc_resourcelist` with no custom (spray) resources.
pub fn empty_resource_list(w: &mut MsgWriter) {
    w.write_u8(op::RESOURCELIST);
    w.write_i16(0);
}

/// `clc_move`: `commands` oldest first (`num_backup` backups, then the new commands).
/// The payload after the checksum byte is CRC'd in plaintext, then munged with the sequence.
pub fn write_move(w: &mut MsgWriter, usercmd: &Delta, commands: &[UserCmd], num_backup: u8, loss: u8, outgoing_sequence: u32) {
    let start = w.len();
    w.write_u8(op::MOVE);
    w.write_u8(0);
    w.write_u8(0);
    w.write_u8(loss);
    w.write_u8(num_backup);
    w.write_u8((commands.len() - usize::from(num_backup)) as u8);
    let mut prev = UserCmd::default().to_bytes();
    for cmd in commands {
        let bytes = cmd.to_bytes();
        w.start_bits();
        usercmd.encode(w, &prev, &bytes, true, 0.0);
        w.end_bits();
        prev = bytes;
    }
    let body = start + 3;
    let len = (w.len() - body).min(255);
    let buf = w.as_bytes_mut();
    buf[start + 1] = len as u8;
    buf[start + 2] = block_sequence_crc_byte(&buf[body..], outgoing_sequence);
    munge(&mut buf[body..body + len], outgoing_sequence as i32, MungeTable::One);
}

/// `COM_BlockSequenceCRCByte`: CRC-32 of up to 60 bytes plus 4 bytes of the CRC table itself
/// (byte view) at `sequence % 1020`; the low byte of the result.
pub fn block_sequence_crc_byte(data: &[u8], sequence: u32) -> u8 {
    let at = (sequence % 1020) as usize;
    let table_bytes = CRC32_TABLE[at / 4..at / 4 + 2].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
    let mut crc = Crc32::default();
    crc.update(&data[..data.len().min(60)]);
    crc.update(&table_bytes[at % 4..at % 4 + 4]);
    crc.finish() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitbuf::MsgReader;
    use crate::delta::{DeltaField, dt, size};
    use gs_net::munge::unmunge;

    #[test]
    fn move_packet_layout_round_trips() {
        let f = |kind, name: &str, offset, bits| DeltaField { kind, name: name.into(), offset, size: 1, bits, pre: 1.0, post: 1.0, recv: Default::default() };
        let delta = Delta {
            name: "usercmd_t".into(),
            fields: vec![f(dt::BYTE, "msec", 2, 8), f(dt::FLOAT | dt::SIGNED, "forwardmove", 16, 12)],
        };
        let cmds = [
            UserCmd { msec: 10, ..Default::default() },
            UserCmd { msec: 16, forwardmove: 250.0, ..Default::default() },
        ];
        let mut w = MsgWriter::new();
        write_move(&mut w, &delta, &cmds, 1, 5, 1234);
        let mut bytes = w.into_bytes();
        assert_eq!(bytes[0], op::MOVE);
        let len = usize::from(bytes[1]);
        assert_eq!(len, bytes.len() - 3);
        unmunge(&mut bytes[3..3 + len], 1234, MungeTable::One);
        assert_eq!(block_sequence_crc_byte(&bytes[3..], 1234), bytes[2], "server-side checksum check");
        assert_eq!(&bytes[3..6], &[5, 1, 1], "loss, backup, new");
        let mut r = MsgReader::new(&bytes[6..]);
        let mut prev = [0u8; size::USERCMD];
        for cmd in &cmds {
            let mut out = [0u8; size::USERCMD];
            r.start_bits();
            delta.decode(&mut r, &prev, &mut out, 0.0);
            r.end_bits();
            assert_eq!(UserCmd::from_bytes(&out), *cmd);
            prev = out;
        }
        assert!(r.at_end());
    }
}
