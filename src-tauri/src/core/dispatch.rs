//! Name-and-JSON dispatch over `core`'s command functions.
//!
//! This is what lets one `rpc` Tauri command stand in for all of them, and
//! what will let the daemon serve an `Rpc::Invoke { command, args }` frame
//! without a webview in the process ([DEVELOPMENT.md §12](../../../DEVELOPMENT.md)).
//!
//! **What this takes over from Tauri, and the risk that comes with it.**
//! `#[tauri::command]` generates the argument deserialization, including
//! mapping the frontend's camelCase (`recordingId`) onto Rust's snake_case
//! (`recording_id`). A passthrough owns that instead, and a mismatch is a
//! *runtime* failure rather than a compile error. Two things hold it down:
//! the table below is the single place any of it is written, and
//! `every_command_round_trips` in the tests exercises every entry with a
//! representative payload, so a wrong name or type fails `cargo test`.
//!
//! The *call* is still compile-checked — each arm calls the real function,
//! so arity and types cannot drift from `core`.

use super::Ctx;
use serde_json::Value;

/// Expands one table row into its `match` arm.
///
/// The leading token picks the shape, because the commands are not uniform:
/// most take `&Ctx` and return `Result`, two return a plain value, and two
/// take no context at all (one of those is the only async command).
macro_rules! invoke_one {
    (ctx_result $name:ident, $ctx:expr, $a:expr, $($arg:ident,)*) => {
        serde_json::to_value(super::$name($ctx, $($a.$arg,)*)?).map_err(|e| e.to_string())
    };
    (ctx_plain $name:ident, $ctx:expr, $a:expr, $($arg:ident,)*) => {
        serde_json::to_value(super::$name($ctx, $($a.$arg,)*)).map_err(|e| e.to_string())
    };
    (bare_result $name:ident, $ctx:expr, $a:expr,) => {
        serde_json::to_value(super::$name()?).map_err(|e| e.to_string())
    };
    (bare_async $name:ident, $ctx:expr, $a:expr,) => {
        serde_json::to_value(super::$name().await).map_err(|e| e.to_string())
    };
}

/// The same rows, for the synchronous entry point. An async command has no
/// blocking form, so it reports that rather than being silently unreachable.
macro_rules! invoke_one_blocking {
    (ctx_result $name:ident, $ctx:expr, $a:expr, $($arg:ident,)*) => {
        serde_json::to_value(super::$name($ctx, $($a.$arg,)*)?).map_err(|e| e.to_string())
    };
    (ctx_plain $name:ident, $ctx:expr, $a:expr, $($arg:ident,)*) => {
        serde_json::to_value(super::$name($ctx, $($a.$arg,)*)).map_err(|e| e.to_string())
    };
    (bare_result $name:ident, $ctx:expr, $a:expr,) => {
        serde_json::to_value(super::$name()?).map_err(|e| e.to_string())
    };
    (bare_async $name:ident, $ctx:expr, $a:expr,) => {
        Err(format!("{} is async and must go through dispatch()", stringify!($name)))
    };
}

macro_rules! is_async_arm {
    (bare_async) => { true };
    ($other:ident) => { false };
}

macro_rules! dispatch_table {
    ($( $kind:ident $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) ; )*) => {
        /// Every command this dispatcher answers to, in table order.
        ///
        /// The dev portal's drift banner reads this instead of a hand-written
        /// list, so the Rust side of that check can no longer drift by
        /// construction — the names and the `match` arms come from the same
        /// macro invocation.
        ///
        /// Only `dev_registered_commands` and this module's tests call this,
        /// and clippy runs without `--all-targets`, so in a build without
        /// `devtools` it is genuinely dead code and `-D warnings` would fail
        /// on it. Same shape as `RecorderError::Backend`'s allow.
        #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
        pub fn command_names() -> &'static [&'static str] {
            &[ $( stringify!($name), )* ]
        }

        /// Whether a command has to be awaited rather than run on a blocking
        /// thread. Callers need this to put each command on the right kind of
        /// thread; see `dispatch_blocking`.
        pub fn is_async_command(command: &str) -> bool {
            match command {
                $( stringify!($name) => is_async_arm!($kind), )*
                _ => false,
            }
        }

        /// Runs one command by name. `args` is the frontend's argument object;
        /// `null` and `{}` are both accepted for a command that takes none.
        ///
        /// Async because `lcu_status` is. Everything else here is *blocking*
        /// work — SQLite, a directory scan, ffmpeg — so a caller that awaits
        /// this on an async worker is occupying that worker for the duration.
        /// Prefer `dispatch_blocking` on a blocking thread for anything
        /// `is_async_command` says no to.
        pub async fn dispatch(ctx: &Ctx, command: &str, args: Value) -> Result<Value, String> {
            let args = normalize(args);
            match command {
                $(
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Args { $( $arg: $ty, )* }
                        #[allow(unused_variables)]
                        let parsed: Args = parse::<Args>(args, stringify!($name))?;
                        invoke_one!($kind $name, ctx, parsed, $($arg,)*)
                    }
                )*
                other => Err(format!("unknown command: {other}")),
            }
        }

        /// The synchronous half, for running on a thread that is allowed to
        /// block. Identical to `dispatch` except that the one async command
        /// refuses rather than pretending.
        pub fn dispatch_blocking(ctx: &Ctx, command: &str, args: Value) -> Result<Value, String> {
            let args = normalize(args);
            match command {
                $(
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Args { $( $arg: $ty, )* }
                        #[allow(unused_variables)]
                        let parsed: Args = parse::<Args>(args, stringify!($name))?;
                        invoke_one_blocking!($kind $name, ctx, parsed, $($arg,)*)
                    }
                )*
                other => Err(format!("unknown command: {other}")),
            }
        }
    };
}

/// A command taking no arguments is invoked with `null` by some callers and
/// `{}` by others; both have to mean "no arguments".
fn normalize(args: Value) -> Value {
    if args.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        args
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value, command: &str) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments for {command}: {e}"))
}

// The single source of truth for the command surface. Adding a command means
// adding a row here and an entry in `src/dev/registry.ts` (which carries help
// text and arg specs a macro can't produce) — the two hand-written
// `generate_handler!` lists and `dev_registered_commands` no longer repeat it.
dispatch_table! {
    ctx_result  start_recording();
    ctx_result  stop_recording();
    ctx_result  is_recording();
    ctx_result  list_recordings();
    ctx_result  rescan_recordings();
    ctx_result  get_recording_markers(recording_id: i64);
    ctx_result  get_recording_samples(recording_id: i64);
    ctx_result  get_disk_usage();
    ctx_result  get_retention_policy();
    ctx_result  set_retention_policy(policy: crate::db::RetentionPolicy);
    ctx_result  set_pinned(recording_id: i64, pinned: bool);
    ctx_result  preview_retention_policy(policy: crate::db::RetentionPolicy);
    ctx_result  delete_recording(recording_id: i64);
    ctx_plain   get_recordings_dir();
    ctx_result  get_ui_prefs();
    ctx_result  set_ui_pref(key: String, value: String);
    ctx_result  get_audio_preset();
    ctx_result  set_audio_preset(preset: crate::recorder::audio::AudioPreset);
    bare_result list_audio_inputs();
    ctx_result  extract_audio_track(recording_path: String, track_index: usize);
    bare_async  lcu_status();
    ctx_plain   game_state_status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::recorder::stub::StubRecorder;
    use crate::recorder::Recorder;
    use crate::state_machine;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn ctx() -> Ctx {
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let db = Arc::new(Db::open_in_memory().unwrap());
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-dispatch-test-{}",
            std::process::id()
        ));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Ctx::new(recorder, supervisor, db, dir, None)
    }

    /// A representative argument payload per command, in the **camelCase the
    /// frontend actually sends** — `bridge.ts` passes its args object through
    /// untouched, so this is the real wire shape.
    fn sample_args(name: &str) -> Value {
        match name {
            "get_recording_markers" | "get_recording_samples" | "delete_recording" => {
                json!({ "recordingId": 1 })
            }
            "set_pinned" => json!({ "recordingId": 1, "pinned": true }),
            "set_retention_policy" | "preview_retention_policy" => json!({
                "policy": { "max_total_bytes": null, "max_age_days": null }
            }),
            "set_ui_pref" => json!({ "key": "theme", "value": "dark" }),
            // Internally tagged on `preset`, so the value is an object.
            "set_audio_preset" => json!({ "preset": { "preset": "game" } }),
            "extract_audio_track" => json!({ "recordingPath": "/tmp/nope.mp4", "trackIndex": 1 }),
            _ => json!({}),
        }
    }

    /// The whole point of the table: every command must be reachable by name
    /// with the arguments the frontend really sends, and must not fail
    /// *argument parsing*. A command is free to return a business error — a
    /// missing recording, no ffmpeg — but "bad arguments for …" means the
    /// camelCase mapping or a type is wrong, which is exactly the class of bug
    /// the Tauri macro used to catch at compile time.
    #[tokio::test]
    async fn every_command_round_trips() {
        let ctx = ctx();
        for name in command_names() {
            let result = dispatch(&ctx, name, sample_args(name)).await;
            if let Err(e) = &result {
                assert!(
                    !e.starts_with("bad arguments for"),
                    "{name}: argument mapping is wrong -- {e}"
                );
                assert!(!e.starts_with("unknown command"), "{name}: not reachable");
            }
        }
    }

    #[tokio::test]
    async fn a_command_taking_no_arguments_accepts_null_and_empty() {
        let ctx = ctx();
        for args in [Value::Null, json!({})] {
            let dir = dispatch(&ctx, "get_recordings_dir", args).await.unwrap();
            assert!(dir.is_string());
        }
    }

    #[tokio::test]
    async fn arguments_are_read_as_camel_case() {
        let ctx = ctx();
        // snake_case is what the Rust signature uses and what serde would
        // accept without the rename — it must NOT be what the wire expects,
        // or the rename is silently doing nothing.
        let err = dispatch(&ctx, "set_pinned", json!({ "recording_id": 1, "pinned": true }))
            .await
            .expect_err("snake_case keys should not satisfy a camelCase struct");
        assert!(err.starts_with("bad arguments for set_pinned"), "{err}");

        dispatch(&ctx, "set_pinned", json!({ "recordingId": 1, "pinned": true }))
            .await
            .expect("camelCase keys should parse");
    }

    /// The camelCase rename applies to the *argument names* only. Types
    /// nested inside an argument keep whatever serde attributes they declare
    /// — `RetentionPolicy` is plain snake_case, and `src/types.ts` mirrors it
    /// that way. Asserted on the values that come back out, not just on
    /// "it parsed", because of the caveat below.
    ///
    /// **Caveat, and it is not new here.** `RetentionPolicy`'s fields are
    /// `Option`, and serde defaults a missing `Option` to `None` rather than
    /// erroring. So a mis-cased nested key is silently dropped and reads as
    /// "unbounded" instead of failing — which the `#[tauri::command]` macro
    /// did too, since it also just called `serde_json`. Worth knowing when
    /// adding a nested argument type; `deny_unknown_fields` on the inner type
    /// is the fix if it ever matters.
    #[tokio::test]
    async fn nested_types_keep_their_own_casing() {
        let ctx = ctx();
        dispatch(
            &ctx,
            "set_retention_policy",
            json!({ "policy": { "max_total_bytes": 123, "max_age_days": 7 } }),
        )
        .await
        .expect("snake_case fields inside an argument should parse");

        let saved = dispatch(&ctx, "get_retention_policy", Value::Null)
            .await
            .unwrap();
        assert_eq!(saved["max_total_bytes"], 123, "value did not survive");
        assert_eq!(saved["max_age_days"], 7, "value did not survive");

        // camelCase does not reach the nested type: the keys are ignored and
        // both fields fall back to `None`, i.e. "unbounded".
        dispatch(
            &ctx,
            "set_retention_policy",
            json!({ "policy": { "maxTotalBytes": 999, "maxAgeDays": 99 } }),
        )
        .await
        .unwrap();
        let saved = dispatch(&ctx, "get_retention_policy", Value::Null)
            .await
            .unwrap();
        assert!(
            saved["max_total_bytes"].is_null() && saved["max_age_days"].is_null(),
            "camelCase should not have reached the nested type, got {saved}"
        );
    }

    #[test]
    fn close_action_round_trips_through_its_pref_string() {
        for action in [
            super::super::CloseAction::CloseWindow,
            super::super::CloseAction::Hide,
            super::super::CloseAction::Quit,
        ] {
            assert_eq!(
                super::super::CloseAction::from_pref(Some(action.as_pref())),
                action
            );
        }
    }

    #[test]
    fn an_unset_or_unrecognised_close_action_falls_back_to_the_default() {
        use super::super::CloseAction;
        assert_eq!(CloseAction::from_pref(None), CloseAction::default());
        assert_eq!(CloseAction::from_pref(Some("")), CloseAction::default());
        // A value written by a *newer* build. `settings_kv` is schemaless and
        // shared across versions, so a downgrade must not brick the close
        // button.
        assert_eq!(CloseAction::from_pref(Some("exit-ui")), CloseAction::default());
    }

    #[test]
    fn the_default_close_action_keeps_the_process_alive() {
        // The window closing must not stop a recording; that is the whole
        // premise of the tray. If this default ever becomes `Quit`, closing
        // the window mid-game would silently end the recording.
        assert_ne!(super::super::CloseAction::default(), super::super::CloseAction::Quit);
    }

    #[tokio::test]
    async fn close_action_reads_back_what_set_ui_pref_wrote() {
        let ctx = ctx();
        assert_eq!(super::super::close_action(&ctx), super::super::CloseAction::default());
        dispatch(
            &ctx,
            "set_ui_pref",
            json!({ "key": super::super::CLOSE_ACTION_KEY, "value": "hide" }),
        )
        .await
        .unwrap();
        assert_eq!(super::super::close_action(&ctx), super::super::CloseAction::Hide);
    }

    #[tokio::test]
    async fn an_unknown_command_is_an_error_not_a_panic() {
        let ctx = ctx();
        let err = dispatch(&ctx, "nope", json!({})).await.unwrap_err();
        assert!(err.contains("unknown command"), "{err}");
    }

    #[tokio::test]
    async fn results_serialize_to_the_shape_the_frontend_expects() {
        let ctx = ctx();
        let usage = dispatch(&ctx, "get_disk_usage", Value::Null).await.unwrap();
        for key in ["total_bytes", "recording_count", "free_bytes"] {
            assert!(usage.get(key).is_some(), "get_disk_usage missing {key}");
        }
        let rows = dispatch(&ctx, "list_recordings", Value::Null).await.unwrap();
        assert!(rows.is_array());
    }
}
