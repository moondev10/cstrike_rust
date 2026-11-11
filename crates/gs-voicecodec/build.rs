//! Compiles the vendored codecs: libopus 1.5.2 (`opus/`, the `v1.5.2` tag of
//! github.com/xiph/opus, the files of its `*_sources.mk`/`*_headers.mk` for a float build) without
//! the DNN extensions or run-time CPU dispatch, and Skype's floating-point SILK SDK 1.0.9
//! (`silk/`: the SDK package's `src` and `interface`, as in ReVoice's `external/silk` but
//! without its edit to `SKP_Silk_wrappers_FLP.c`, which misplaces the second set of prediction
//! coefficients the noise-shaping quantizer reads) that decodes Steam's legacy SILK payload.
//!
//! Local edits to the SILK decoder, marked `gs-voicecodec:` (it decodes any player's voice):
//! pitch lags are limited to the sampling rate's range when decoded, at the re-whitening and in
//! the concealment, and a rate change resets the concealment state, as libopus' SILK does. A
//! stream switching rates around a lost frame otherwise carried a 24 kHz lag into an 8 kHz frame
//! and wrote before `sLTP` on the stack in `SKP_Silk_decode_core` (found with ASan).

use std::io;
use std::path::{Path, PathBuf};

/// Every `.c` below `dir`, sorted (the vendored trees hold only what is compiled).
fn c_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "c") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The vendored code's own warnings are not ours to fix: silenced, not forwarded to cargo.
fn quiet(build: &mut cc::Build) -> &mut cc::Build {
    let msvc = build.get_compiler().is_like_msvc();
    build.opt_level(2).warnings(false).cargo_warnings(false).flag(if msvc { "/W0" } else { "-w" })
}

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-changed=opus");
    println!("cargo::rerun-if-changed=silk");

    let mut opus = cc::Build::new();
    quiet(&mut opus)
        .files(c_files(Path::new("opus"))?)
        .includes(["opus/include", "opus/celt", "opus/silk", "opus/silk/float", "opus/src"])
        .define("OPUS_BUILD", None)
        .define("USE_ALLOCA", None)
        .define("DISABLE_DEBUG_FLOAT", None)
        .compile("opus");

    let mut silk = cc::Build::new();
    quiet(&mut silk).files(c_files(Path::new("silk/src"))?).includes(["silk/include/silk", "silk/src"]).compile("silk");
    Ok(())
}
