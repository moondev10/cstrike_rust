//! The GoldSrc engine sound system (docs/re/audio.md): WAV loading, sentences, dynamic and
//! static channels with spatialization, the 11025/22050 Hz software mixer, the room DSP, the
//! voice engine (docs/re/voice-engine.md), MP3 music on its own stream (docs/re/music.md), and
//! a cpal output device.

pub mod device;
pub mod dsp;
pub mod mixer;
pub mod music;
pub mod voice;
pub mod vox;
pub mod wav;

pub use device::Device;
pub use mixer::{Listener, Settings, SfxId, SoundSystem, chan, flags};
