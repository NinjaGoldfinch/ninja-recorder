//! Name-and-JSON dispatch over the `dev_*` commands. WS3 task 3.7.
//!
//! The portal-side twin of `core::dispatch`, and it exists for the same reason
//! that one does: a command reached by name is a command the daemon can run,
//! and a command registered with `generate_handler!` is one only a process with
//! a webview can. Every panel that reads the database, drives the state machine
//! or inspects the log needs the process that owns those, which since WS3.2 is
//! the daemon.
//!
//! ## Why a second dispatcher rather than rows in `core`'s table
//!
//! `core::dispatch`'s table is the production surface: it compiles into every
//! build, and `gen-contract` emits a typed TypeScript client from it. The
//! `dev_*` commands are behind the `devtools` feature and must stay out of a
//! shipped binary entirely, which a `#[cfg]` on forty rows of a macro table
//! cannot express cleanly — `command_names()` builds a `&'static` array, and an
//! array element cannot carry a `cfg` attribute on stable Rust.
//!
//! A whole module behind the feature says it once, in the place the rest of the
//! dev surface already lives. `core::dispatch` delegates to it with a single
//! `cfg`-gated arm, so the production table stays exactly what it was.
//!
//! ## The arms come from the declaration
//!
//! `contract::portal`'s `dev_rpc_command_table!` carries each command's Rust
//! signature in its `call:` field, and this module expands that into the
//! `match`. The function is called by the macro, so its arity and types are
//! compile-checked against the table: getting a row wrong is an error at the
//! call site rather than a manifest that lies.
//!
//! What is *not* compile-checked is argument **naming**, exactly as in
//! `core::dispatch`: serde maps the wire's camelCase onto the generated `Args`
//! struct at runtime. `every_dev_command_round_trips` is the cover for that,
//! and it is the reason this module has tests at all.

use serde_json::Value;

use crate::core::Ctx;

/// Invokes one command, parsing `args` into whatever the table says it takes.
macro_rules! invoke_dev {
    (ctx_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*)?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (ctx_plain $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*);
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (ctx_async_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*).await?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($($a.$arg,)*)?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_plain $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($($a.$arg,)*);
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_async_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($($a.$arg,)*).await?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
}

/// The same rows for the blocking entry point. An async command has no
/// blocking form and says so rather than being silently unreachable, exactly as
/// `core::dispatch` does.
macro_rules! invoke_dev_blocking {
    (ctx_async_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        $( let _ = &$a.$arg; )*
        Err(format!("{} is async and must go through dispatch()", stringify!($name)))
    }};
    (bare_async_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        $( let _ = &$a.$arg; )*
        Err(format!("{} is async and must go through dispatch()", stringify!($name)))
    }};
    ($kind:ident $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {
        invoke_dev!($kind $name, $ret, $ctx, $a, $($arg,)*)
    };
}

macro_rules! is_async_dev {
    (ctx_async_result) => { true };
    (bare_async_result) => { true };
    ($other:ident) => { false };
}

macro_rules! dev_dispatch {
    ($(
        $(#[doc = $doc:literal])*
        $name:ident {
            group: $group:literal,
            danger: $danger:literal,
            call: $kind:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty,
            args: [ $( { $($form:tt)* } ),* $(,)? ],
        }
    )*) => {
        /// Whether this command has to be awaited.
        ///
        /// The portal's own commands are mostly blocking work against SQLite or
        /// the filesystem; the handful that are async talk to the League client
        /// over HTTP. Running the first kind on an async worker would occupy it
        /// for the length of a query, which is what `spawn_blocking` exists to
        /// avoid.
        pub fn is_async_dev_command(command: &str) -> bool {
            match command {
                $( stringify!($name) => is_async_dev!($kind), )*
                _ => false,
            }
        }

        /// Runs one `dev_*` command. The async entry point.
        pub async fn dispatch_dev(ctx: &Ctx, command: &str, args: Value) -> Result<Value, String> {
            match command {
                $(
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        #[allow(non_snake_case, dead_code)]
                        struct Args { $( $arg: $ty, )* }
                        // `unused_variables` because a command with no
                        // arguments still parses an empty `Args`, which nothing
                        // then reads. The same allow, for the same reason, as
                        // `core::dispatch`'s.
                        #[allow(unused_variables)]
                        let a: Args = parse::<Args>(default_object(args), command)?;
                        invoke_dev!($kind $name, $ret, ctx, a, $($arg,)*)
                    }
                )*
                _ => Err(format!("unknown dev command: {command}")),
            }
        }

        /// And the blocking one, for the commands that are blocking work.
        pub fn dispatch_dev_blocking(ctx: &Ctx, command: &str, args: Value) -> Result<Value, String> {
            match command {
                $(
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        #[allow(non_snake_case, dead_code)]
                        struct Args { $( $arg: $ty, )* }
                        // `unused_variables` because a command with no
                        // arguments still parses an empty `Args`, which nothing
                        // then reads. The same allow, for the same reason, as
                        // `core::dispatch`'s.
                        #[allow(unused_variables)]
                        let a: Args = parse::<Args>(default_object(args), command)?;
                        invoke_dev_blocking!($kind $name, $ret, ctx, a, $($arg,)*)
                    }
                )*
                _ => Err(format!("unknown dev command: {command}")),
            }
        }
    };
}

/// A command with no arguments is invoked with `null` by some callers and `{}`
/// by others; both have to parse into an empty `Args`. Same helper, same
/// reason, as `core::dispatch`'s.
fn default_object(args: Value) -> Value {
    if args.is_null() { Value::Object(serde_json::Map::new()) } else { args }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value, command: &str) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments for {command}: {e}"))
}

crate::dev_rpc_command_table!(dev_dispatch);

#[cfg(test)]
mod tests {
    //! The cover for the one thing the compiler cannot check here.
    //!
    //! Each arm's *call* is compile-checked: the macro calls the real function,
    //! so arity and types cannot drift from `dev/`. What is not checked is the
    //! **name** each argument arrives under, because serde maps the wire's
    //! camelCase onto the generated `Args` struct at runtime. A `recordingId`
    //! the table spells `recording_id` compiles perfectly and fails when a
    //! person clicks the button.
    //!
    //! So every command is invoked with a representative payload, exactly as
    //! `core::dispatch`'s `every_command_round_trips` does. Most of them then
    //! fail for ordinary reasons — no League client, no such recording, an
    //! invalid fixture group — and that is fine: what is asserted is that they
    //! were *reached* and their arguments *parsed*.

    use super::*;
    use crate::contract::portal::dev_rpc_command_names;
    use crate::db::Db;
    use crate::recorder::Recorder;
    use crate::recorder::stub::StubRecorder;
    use crate::state_machine;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn ctx() -> Ctx {
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let db = Arc::new(Db::open_temporary().unwrap());
        let dir = std::env::temp_dir()
            .join(format!("ninja-recorder-dev-dispatch-{}", std::process::id()));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Ctx::new(recorder, supervisor, db, dir.clone(), dir.join("ddragon"), None)
    }

    /// A payload for each command that takes arguments.
    ///
    /// Chosen to be **reached and refused** rather than to succeed: the
    /// commands that write pick inputs their own validation rejects, and the
    /// ones that talk to the League client are exercised on a machine where
    /// there is none. A test that made `dev_reset_db` actually wipe something
    /// or `dev_replay_start` actually spawn a game would be testing the
    /// command, which is not what this is for.
    fn sample_args(name: &str) -> Value {
        match name {
            "dev_backfill_recording" | "dev_recording_report" | "dev_recording_vs_lcu"
            | "dev_trim_lead_in" => json!({ "recordingId": 1 }),
            "dev_champion_name" => json!({ "championId": 1 }),
            "dev_fetch_match_summary" => json!({ "gameId": 1 }),
            "dev_patch_match_summary" => json!({
                "recordingId": 1, "gameId": 1, "isCustom": false, "queueId": null
            }),
            "dev_lcu_get" | "dev_event_uris" | "dev_fixture_read" => {
                json!({ "path": "nothing/here" })
            }
            // An unknown group, so the write is refused before it reaches a
            // directory. This command creates files.
            "dev_fixture_write" => {
                json!({ "group": "not-a-group", "name": "x.json", "contents": "{}" })
            }
            "dev_set_fixture_recording" => json!({ "enabled": false }),
            "dev_read_log" => json!({ "query": {
                "file": null, "levels": [], "hideTags": [], "search": "", "limit": 1
            }}),
            "dev_table_page" => {
                json!({ "table": "recordings", "limit": 1, "offset": 0, "orderBy": null })
            }
            "dev_sql_query" => json!({ "sql": "SELECT 1" }),
            "dev_insert_row" => json!({ "table": "recordings", "values": {} }),
            "dev_update_row" => json!({ "table": "recordings", "id": 1, "values": {} }),
            "dev_delete_row" => json!({ "table": "recordings", "id": 1, "deleteFile": false }),
            // `false`, so the temporary library is emptied and nothing on disk
            // is touched.
            "dev_reset_db" => json!({ "alsoClearFiles": false }),
            // Nothing to seed: the arguments are what is under test, and a
            // count above zero would write media files.
            // Snake_case inside, camelCase outside, and that is not an
            // inconsistency to tidy: `rename_all = "camelCase"` is on the
            // `Args` wrapper this macro generates, while `SeedSpec` carries its
            // own serde config and is spelled the way the seed panel already
            // sends it.
            "dev_seed_library" => json!({ "spec": {
                "count": 0, "duration_min_s": 1.0, "duration_max_s": 2.0,
                "markers_min": 0, "markers_max": 0, "samples": false,
                "file_bytes": 0, "use_sample_mp4": false, "spread_days": 1,
                "pinned_every": 0, "messy": false, "seed": 1
            }}),
            "dev_retention_preview" => json!({
                "policy": { "max_total_bytes": null, "max_age_days": null },
                "nowMillis": null
            }),
            // `snake_case` on the tag, as `DevStateEvent` declares and as the
            // simulate panel sends.
            "dev_dispatch_state_event" => json!({ "event": { "kind": "lockfile_absent" } }),
            "dev_inject_snapshot" => json!({ "snapshot": {} }),
            // An empty base payload, which `dev_replay_start` validates and
            // rejects before spawning anything.
            "dev_replay_start" => json!({ "spec": {
                "base_snapshot": {}, "duration_s": 1.0, "speed": 1.0
            }}),
            "dev_ranked_stats" => json!({ "puuid": null }),
            "dev_lobby_rank" => json!({ "puuids": [], "queue": null }),
            "dev_lp_delta" => json!({
                "before": { "tier": "GOLD", "division": "II", "leaguePoints": 10 },
                "after": { "tier": "GOLD", "division": "II", "leaguePoints": 30 },
                "queue": null
            }),
            _ => json!({}),
        }
    }

    #[tokio::test]
    async fn every_dev_command_round_trips() {
        let ctx = ctx();
        for name in dev_rpc_command_names() {
            let result = dispatch_dev(&ctx, name, sample_args(name)).await;
            if let Err(e) = &result {
                assert!(
                    !e.starts_with("bad arguments for"),
                    "{name}: argument mapping is wrong -- {e}"
                );
                assert!(!e.starts_with("unknown dev command"), "{name}: not reachable");
            }
        }
    }

    /// The blocking entry point reaches the same commands, and refuses the
    /// async ones by name rather than by falling through to "unknown".
    #[test]
    fn the_blocking_entry_point_covers_the_same_surface() {
        let ctx = ctx();
        for name in dev_rpc_command_names() {
            let result = dispatch_dev_blocking(&ctx, name, sample_args(name));
            if let Err(e) = &result {
                assert!(!e.starts_with("unknown dev command"), "{name}: not reachable");
                if is_async_dev_command(name) {
                    assert!(
                        e.contains("is async and must go through dispatch()"),
                        "{name}: an async command should say so -- {e}"
                    );
                }
            }
        }
    }

    /// #282: the Overview and Recorder panels read the recorder from here,
    /// so it has to be the recorder of the process answering, with the file
    /// it is writing while it writes one.
    #[tokio::test]
    async fn health_reports_the_recorder_of_the_process_that_owns_it() {
        let ctx = ctx();
        let idle = dispatch_dev(&ctx, "dev_health", json!({})).await.unwrap();
        assert_eq!(idle["is_recording"], json!(false));
        assert_eq!(idle["recorder"]["backend"], json!("stub"));
        // A fresh library has nothing saved; the daemon picks (#243).
        assert_eq!(idle["recorder"]["configured"], json!("unset"));
        assert_eq!(idle["recorder"]["current_file"], Value::Null);
        // The stub has no worker, which is not the same as a worker that is down.
        assert_eq!(idle["recorder"]["worker_running"], Value::Null);

        crate::core::start_recording(&ctx).unwrap();
        let busy = dispatch_dev(&ctx, "dev_health", json!({})).await.unwrap();
        assert_eq!(busy["is_recording"], json!(true));
        let file = busy["recorder"]["current_file"].as_str().expect("a file while recording");
        assert!(file.ends_with(".mp4"), "{file}");
        assert!(file.starts_with(&ctx.recordings_dir.display().to_string()), "{file}");

        let saved = crate::core::stop_recording(&ctx).unwrap();
        assert_eq!(saved, file);
        std::fs::remove_file(&saved).ok();
    }

    /// The portal reaches these by name over `rpc`, so a name that is not
    /// prefixed would be routed to `core`'s table and reported as unknown.
    #[test]
    fn every_dispatched_name_is_dev_prefixed() {
        for name in dev_rpc_command_names() {
            assert!(name.starts_with("dev_"), "{name} would not route to this dispatcher");
        }
    }
}
