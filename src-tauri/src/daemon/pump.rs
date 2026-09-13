//! Win32 message loop and tray icon for the daemon.
//!
//! **Empty — WS3 task 3.3.** v1's `tray.rs` builds the tray on Tauri's event
//! loop; the daemon has no Tauri, so this becomes a `tray-icon` menu driven by
//! a plain `GetMessage`/`DispatchMessage` pump. Autostart and the update check
//! run on the same thread, because both want the foreground session's message
//! queue (implementation plan §3.1, §4.3).
