fn main() {
    // The libobs runtime (extprocess_recorder.exe + DLLs/plugins) is staged
    // into target/libobs/ by a separate step outside Cargo's build graph —
    // see the "Stage libobs capture backend" CI step and DEVELOPMENT.md
    // §2.1/§9 for why. `tauri.windows.conf.json` bundles that folder into
    // the installer; `LibObsRecorder::new` (lib.rs) resolves it at runtime.
    tauri_build::build()
}
