fn main() {
    // tauri_build::build() validates that every path in
    // tauri.windows.conf.json's `bundle.resources` exists — unconditionally,
    // even for a plain `cargo build`/`test`/`clippy` that never bundles
    // anything. The real contents of target/libobs/ (extprocess_recorder.exe
    // + DLLs/plugins) are staged by a separate step outside Cargo's build
    // graph — see the "Stage libobs capture backend" CI step and
    // DEVELOPMENT.md §2.1/§9 — which only runs before actual packaging, so
    // an empty placeholder here keeps every other Windows build (CI's test
    // job, a plain `cargo check`) from failing on a resource path that
    // doesn't need real content yet. `LibObsRecorder::new` (lib.rs) resolves
    // the real files at runtime, once they've actually been staged.
    #[cfg(target_os = "windows")]
    std::fs::create_dir_all("target/libobs")
        .expect("failed to create target/libobs placeholder directory");

    delay_load_media_foundation();

    tauri_build::build()
}

/// Media Foundation's DLLs are loaded on first use rather than at startup.
///
/// The own capture backend (`recorder/own/win/`) calls into `mfplat.dll`
/// (since #239 retired the sink writer, the only Media Foundation DLL it
/// imports from), and an ordinary import would make Windows refuse to
/// start the executable at all where they are missing: a Windows N edition
/// without the Media Feature Pack, or a Server image without the Media
/// Foundation feature. That would take the daemon, the library and the libobs
/// backend down with it. Delay-loaded, the executable starts everywhere and
/// `own::win::device::media_foundation` asks whether it exists before
/// anything calls them.
///
/// Read from the target, not `cfg!`: a build script's `cfg` is the host's.
/// MSVC only, which is the only Windows toolchain this project builds with.
fn delay_load_media_foundation() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        // A `/DELAYLOAD` for a DLL nothing imports from is a linker
        // warning (LNK4199), so this names exactly the ones in use.
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfplat.dll");
        // The helper `/DELAYLOAD` needs, from the MSVC runtime libraries.
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
