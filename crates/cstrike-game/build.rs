//! The main thread's stack. The BSP tree walks (hull traces, light and decal walks) recurse once
//! per tree level, up to the depth `gs-formats` accepts from a map (1024, about 0.5 MiB in a dev
//! build); Windows gives an executable 1 MiB unless its image asks for more. Elsewhere the main
//! thread gets the 8 MiB default of `ulimit -s`.
//!
//! The executable's icon (`icon.rc`): a reticle with the Rust logo, on Windows targets
//! (embed-resource goes by the host, so a cross build for Linux would link a Windows resource).
fn main() -> Result<(), embed_resource::CompilationResult> {
    println!("cargo:rerun-if-changed=icon.rc");
    println!("cargo:rerun-if-changed=icon.ico");
    if !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        return Ok(());
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    } else {
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
    }
    embed_resource::compile("icon.rc", embed_resource::NONE).manifest_optional()
}
