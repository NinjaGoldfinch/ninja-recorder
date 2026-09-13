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

    tauri_build::build()
}
