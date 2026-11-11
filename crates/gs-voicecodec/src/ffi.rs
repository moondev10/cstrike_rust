//! The vendored C codecs (`build.rs`): their entry points and owning wrappers. With
//! `gs-steam/src/ffi.rs`, the only place in the workspace that calls foreign code.
#![allow(unsafe_code)]

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;

/// `OpusEncoder` / `OpusDecoder` (opaque).
#[repr(C)]
struct RawOpusEncoder {
    _private: [u8; 0],
}
#[repr(C)]
struct RawOpusDecoder {
    _private: [u8; 0],
}

const OPUS_OK: c_int = 0;
const OPUS_APPLICATION_VOIP: c_int = 2048;
const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
const OPUS_SET_DTX_REQUEST: c_int = 4016;
const OPUS_SET_SIGNAL_REQUEST: c_int = 4024;
const OPUS_SIGNAL_VOICE: i32 = 3001;
const OPUS_RESET_STATE: c_int = 4028;
const SKP_SILK_NO_ERROR: c_int = 0;

/// `SKP_SILK_SDK_DecControlStruct`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct SilkDecControl {
    api_sample_rate: i32,
    frame_size: c_int,
    frames_per_packet: c_int,
    more_internal_decoder_frames: c_int,
    in_band_fec_offset: c_int,
}

/// `SKP_SILK_SDK_EncControlStruct`.
#[cfg(test)]
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct SilkEncControl {
    api_sample_rate: i32,
    max_internal_sample_rate: i32,
    packet_size: c_int,
    bit_rate: i32,
    packet_loss_percentage: c_int,
    complexity: c_int,
    use_in_band_fec: c_int,
    use_dtx: c_int,
}

unsafe extern "C" {
    fn opus_encoder_create(fs: i32, channels: c_int, application: c_int, error: *mut c_int) -> *mut RawOpusEncoder;
    fn opus_encoder_destroy(st: *mut RawOpusEncoder);
    fn opus_encode(st: *mut RawOpusEncoder, pcm: *const i16, frame_size: c_int, data: *mut u8, max_data_bytes: i32) -> i32;
    fn opus_encoder_ctl(st: *mut RawOpusEncoder, request: c_int, ...) -> c_int;
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut RawOpusDecoder;
    fn opus_decoder_destroy(st: *mut RawOpusDecoder);
    fn opus_decode(st: *mut RawOpusDecoder, data: *const u8, len: i32, pcm: *mut i16, frame_size: c_int, decode_fec: c_int) -> c_int;
    fn opus_decoder_ctl(st: *mut RawOpusDecoder, request: c_int, ...) -> c_int;

    #[link_name = "SKP_Silk_SDK_Get_Decoder_Size"]
    fn silk_get_decoder_size(size: *mut i32) -> c_int;
    #[link_name = "SKP_Silk_SDK_InitDecoder"]
    fn silk_init_decoder(state: *mut c_void) -> c_int;
    #[link_name = "SKP_Silk_SDK_Decode"]
    fn silk_decode(state: *mut c_void, control: *mut SilkDecControl, lost: c_int, data: *const u8, n: c_int, out: *mut i16, n_out: *mut i16) -> c_int;
}

#[cfg(test)]
unsafe extern "C" {
    #[link_name = "SKP_Silk_SDK_Get_Encoder_Size"]
    fn silk_get_encoder_size(size: *mut i32) -> c_int;
    #[link_name = "SKP_Silk_SDK_InitEncoder"]
    fn silk_init_encoder(state: *mut c_void, status: *mut SilkEncControl) -> c_int;
    #[link_name = "SKP_Silk_SDK_Encode"]
    fn silk_encode(state: *mut c_void, control: *const SilkEncControl, samples: *const i16, n: c_int, out: *mut u8, n_bytes: *mut i16) -> c_int;
}

/// A mono `OpusEncoder` set up like Steam's and ReVoice's voice encoders: VoIP, voice signal,
/// DTX.
pub struct OpusEncoder(NonNull<RawOpusEncoder>);

impl OpusEncoder {
    /// `opus_encoder_create(rate, 1, OPUS_APPLICATION_VOIP)` and the bitrate; the Opus error code
    /// on failure.
    pub fn new(rate: i32, bitrate: i32) -> Result<Self, i32> {
        let mut err = OPUS_OK;
        // SAFETY: `err` is a valid out-pointer; a non-null result is owned by `Self` and freed once
        // in `Drop`.
        let st = unsafe { opus_encoder_create(rate, 1, OPUS_APPLICATION_VOIP, &mut err) };
        let st = NonNull::new(st).ok_or(err)?;
        // SAFETY: `st` is a live encoder; each request takes one `opus_int32`.
        unsafe {
            opus_encoder_ctl(st.as_ptr(), OPUS_SET_BITRATE_REQUEST, bitrate);
            opus_encoder_ctl(st.as_ptr(), OPUS_SET_SIGNAL_REQUEST, OPUS_SIGNAL_VOICE);
            opus_encoder_ctl(st.as_ptr(), OPUS_SET_DTX_REQUEST, 1i32);
        }
        Ok(Self(st))
    }

    /// `opus_encode` of one frame (`pcm.len()` must be a legal Opus frame size) into `out`: the
    /// bytes written, or the Opus error code.
    pub fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> Result<usize, i32> {
        let frame = c_int::try_from(pcm.len()).map_err(|_| -1)?;
        let max = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: the encoder is live; `pcm` holds `frame` mono samples and `out` `max` bytes.
        let n = unsafe { opus_encode(self.0.as_ptr(), pcm.as_ptr(), frame, out.as_mut_ptr(), max) };
        usize::try_from(n).map_err(|_| n)
    }

    /// `OPUS_RESET_STATE`.
    pub fn reset(&mut self) {
        // SAFETY: the encoder is live; the request takes no argument.
        unsafe { opus_encoder_ctl(self.0.as_ptr(), OPUS_RESET_STATE) };
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        // SAFETY: created by `opus_encoder_create`, destroyed only here.
        unsafe { opus_encoder_destroy(self.0.as_ptr()) }
    }
}

/// A mono `OpusDecoder`.
pub struct OpusDecoder(NonNull<RawOpusDecoder>);

impl OpusDecoder {
    /// `opus_decoder_create(rate, 1)`; the Opus error code on failure.
    pub fn new(rate: i32) -> Result<Self, i32> {
        let mut err = OPUS_OK;
        // SAFETY: as in `OpusEncoder::new`.
        let st = unsafe { opus_decoder_create(rate, 1, &mut err) };
        NonNull::new(st).map(Self).ok_or(err)
    }

    /// `opus_decode` of one packet into `out`, or packet loss concealment of `out.len()`
    /// samples for `None`: the samples written, or the Opus error code.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [i16]) -> Result<usize, i32> {
        let (data, len) = match packet {
            Some(p) => (p.as_ptr(), i32::try_from(p.len()).map_err(|_| -1)?),
            None => (std::ptr::null(), 0),
        };
        let frame = c_int::try_from(out.len()).unwrap_or(c_int::MAX);
        // SAFETY: the decoder is live; `data` is null or holds `len` bytes; `out` holds `frame`
        // mono samples, the most opus_decode writes.
        let n = unsafe { opus_decode(self.0.as_ptr(), data, len, out.as_mut_ptr(), frame, 0) };
        usize::try_from(n).map_err(|_| n)
    }

    /// `OPUS_RESET_STATE`.
    pub fn reset(&mut self) {
        // SAFETY: the decoder is live; the request takes no argument.
        unsafe { opus_decoder_ctl(self.0.as_ptr(), OPUS_RESET_STATE) };
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: created by `opus_decoder_create`, destroyed only here.
        unsafe { opus_decoder_destroy(self.0.as_ptr()) }
    }
}

/// A SILK SDK state block (malloc-aligned like ReVoice's).
fn state_block(size: i32) -> Vec<u128> {
    vec![0; usize::try_from(size).unwrap_or(0).div_ceil(size_of::<u128>())]
}

/// The most samples one `SKP_Silk_SDK_Decode` call may write to its output: the SDK's own
/// `samplesOutInternal` size, `MAX_API_FS_KHZ * FRAME_LENGTH_MS`. A call writes the frame at the
/// internal rate there before resampling, and picks its scratch by the previous frame's rate,
/// so a stream that switches to 24 kHz writes 480 samples whatever the API rate.
const SILK_CALL_MAX: usize = 48 * 20;

/// A SILK SDK decoder at a fixed output rate.
pub struct SilkDecoder {
    state: Vec<u128>,
    control: SilkDecControl,
    /// Every call's output, copied into the caller's buffer.
    scratch: Box<[i16; SILK_CALL_MAX]>,
}

impl SilkDecoder {
    /// `SKP_Silk_SDK_InitDecoder` with output at `rate` (8000..=48000 Hz); the SILK error code
    /// on failure.
    pub fn new(rate: i32) -> Result<Self, i32> {
        let mut size = 0;
        // SAFETY: `size` is a valid out-pointer.
        unsafe { silk_get_decoder_size(&mut size) };
        let mut d = Self { state: state_block(size), control: SilkDecControl { api_sample_rate: rate, ..SilkDecControl::default() }, scratch: Box::new([0; SILK_CALL_MAX]) };
        match d.reset() {
            SKP_SILK_NO_ERROR => Ok(d),
            e => Err(e),
        }
    }

    /// `SKP_Silk_SDK_InitDecoder`.
    pub fn reset(&mut self) -> i32 {
        // SAFETY: `state` holds the decoder size the SDK reported.
        unsafe { silk_init_decoder(self.state.as_mut_ptr().cast()) }
    }

    /// One SILK payload decoded (`SKP_Silk_SDK_Decode` while it has more internal frames) into
    /// `out`: the samples written. Stops early on a decode error or when `out` cannot take
    /// another 20 ms frame.
    pub fn decode(&mut self, payload: &[u8], out: &mut [i16]) -> usize {
        let Ok(n_in) = c_int::try_from(payload.len()) else { return 0 };
        let frame = usize::try_from(self.control.api_sample_rate / 50).unwrap_or(0);
        let mut written = 0;
        loop {
            if out.len() - written < frame {
                break;
            }
            let mut n: i16 = 0;
            // SAFETY: `state` is an initialised decoder; `payload` holds `n_in` bytes; `scratch`
            // holds the most one call writes (SILK_CALL_MAX).
            let r = unsafe { silk_decode(self.state.as_mut_ptr().cast(), &mut self.control, 0, payload.as_ptr(), n_in, self.scratch.as_mut_ptr(), &mut n) };
            if r != SKP_SILK_NO_ERROR {
                break;
            }
            let n = usize::try_from(n).unwrap_or(0).min(frame);
            out[written..written + n].copy_from_slice(&self.scratch[..n]);
            written += n;
            if self.control.more_internal_decoder_frames == 0 {
                break;
            }
        }
        written
    }
}

/// A SILK SDK encoder (tests build ReVoice's SILK packets with it).
#[cfg(test)]
pub struct SilkEncoder {
    state: Vec<u128>,
    control: SilkEncControl,
}

#[cfg(test)]
impl SilkEncoder {
    /// ReVoice's `VoiceEncoder_Silk` setup: 20 ms packets at `rate`, 25 kbit/s, complexity 2.
    pub fn new(rate: i32) -> Self {
        let mut size = 0;
        let mut status = SilkEncControl::default();
        // SAFETY: `size` is a valid out-pointer.
        unsafe { silk_get_encoder_size(&mut size) };
        let mut state = state_block(size);
        // SAFETY: `state` holds the encoder size the SDK reported; `status` is a valid out-pointer.
        unsafe { silk_init_encoder(state.as_mut_ptr().cast(), &mut status) };
        let control = SilkEncControl { api_sample_rate: rate, max_internal_sample_rate: 16000, packet_size: rate / 50, bit_rate: 25000, complexity: 2, ..SilkEncControl::default() };
        Self { state, control }
    }

    /// `SKP_Silk_SDK_Encode` of 20 ms: the payload bytes.
    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let mut out = vec![0u8; 1024];
        let mut n = 1024i16;
        let len = c_int::try_from(pcm.len()).unwrap_or(0);
        // SAFETY: `state` is an initialised encoder; `pcm` holds `len` samples; `out` holds `n`
        // bytes.
        let r = unsafe { silk_encode(self.state.as_mut_ptr().cast(), &self.control, pcm.as_ptr(), len, out.as_mut_ptr(), &mut n) };
        assert_eq!(r, SKP_SILK_NO_ERROR, "SKP_Silk_SDK_Encode");
        out.truncate(usize::try_from(n).unwrap_or(0));
        out
    }
}
