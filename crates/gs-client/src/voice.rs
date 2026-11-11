//! The engine voice engine as `CL_Move` sees it (docs/re/audio.md §2.9–§2.10): compressed mic
//! data for `clc_voicedata` and the `voice_loopback` bit of the move's loss byte.

/// `Voice_GetCompressedData` + `Voice_GetLoopback`.
pub trait VoiceSend {
    /// Compressed microphone data recorded since the last call into `out` (≤ 2048 bytes); 0
    /// when not recording or nothing is buffered.
    fn compressed(&mut self, out: &mut [u8]) -> usize;
    /// `voice_loopback`: ask the server to echo our own voice.
    fn loopback(&self) -> bool;
}

/// No voice engine (tools, headless runs).
#[derive(Debug, Default)]
pub struct NoVoice;

impl VoiceSend for NoVoice {
    fn compressed(&mut self, _: &mut [u8]) -> usize {
        0
    }

    fn loopback(&self) -> bool {
        false
    }
}
