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

/// One argument of one command, as the generator sees it.
///
/// `ty` is the Rust type spelled exactly as the table spells it. Mapping that
/// onto TypeScript is WS2.5's job, not this module's.
#[cfg_attr(not(test), allow(dead_code))]
pub struct CommandArg {
    pub name: &'static str,
    pub ty: &'static str,
}

/// One argument, with its type already rendered as TypeScript.
///
/// The `&'static str` pair above carries the Rust *spelling*; this carries what
/// that spelling means on the wire. Resolving it needs the real type and a
/// `ts_rs::Config`, neither of which survives into a `&'static` const, which is
/// why this is owned and built on demand.
#[cfg_attr(not(test), allow(dead_code))]
pub struct CommandTsArg {
    /// camelCase, as the wire spells it. See `to_camel`.
    pub name: String,
    pub ts: String,
}

/// One command, rendered for TypeScript.
#[cfg_attr(not(test), allow(dead_code))]
pub struct CommandTsSpec {
    pub name: &'static str,
    pub is_async: bool,
    pub args: Vec<CommandTsArg>,
    pub returns: String,
}

/// `#[serde(rename_all = "camelCase")]` on the generated `Args` struct is what
/// the client has to mirror, so the rule is spelled out once, here, and used by
/// both the generator and `every_command_round_trips`.
///
/// Deliberately not a dependency: this is the whole of the transformation serde
/// applies to these identifiers, which are all plain `snake_case` ASCII.
pub fn to_camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// One command, as the generator sees it. Built by `dispatch_table!` from the
/// same row that builds the `match` arm, so the two cannot disagree.
#[cfg_attr(not(test), allow(dead_code))]
pub struct CommandSpec {
    pub name: &'static str,
    /// Whether it has to be awaited. `dispatch_blocking` refuses these.
    pub is_async: bool,
    pub args: &'static [CommandArg],
    /// The success type. See `contract_manifest` for why this is not the
    /// `Result`.
    pub returns: &'static str,
}

/// Expands one table row into its `match` arm.
///
/// The leading token picks the shape, because the commands are not uniform:
/// most take `&Ctx` and return `Result`, two return a plain value, two take
/// no context at all, and three are async — two of those with a context and
/// one without.
macro_rules! invoke_one {
    (ctx_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*)?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (ctx_plain $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*);
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_result $name:ident, $ret:ty, $ctx:expr, $a:expr,) => {{
        let out: $ret = super::$name()?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_async $name:ident, $ret:ty, $ctx:expr, $a:expr,) => {{
        let out: $ret = super::$name().await;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (ctx_async $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*).await?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
}

/// The same rows, for the synchronous entry point. An async command has no
/// blocking form, so it reports that rather than being silently unreachable.
macro_rules! invoke_one_blocking {
    (ctx_result $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*)?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (ctx_plain $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        let out: $ret = super::$name($ctx, $($a.$arg,)*);
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_result $name:ident, $ret:ty, $ctx:expr, $a:expr,) => {{
        let out: $ret = super::$name()?;
        serde_json::to_value(out).map_err(|e| e.to_string())
    }};
    (bare_async $name:ident, $ret:ty, $ctx:expr, $a:expr,) => {
        Err(format!("{} is async and must go through dispatch()", stringify!($name)))
    };
    // Reads the parsed arguments before refusing. This arm does not invoke
    // anything, and the first async command to take an argument made the
    // generated `Args` field dead code here — which `-D warnings` fails on,
    // from inside a macro, pointing at the table rather than the command.
    (ctx_async $name:ident, $ret:ty, $ctx:expr, $a:expr, $($arg:ident,)*) => {{
        $( let _ = &$a.$arg; )*
        Err(format!("{} is async and must go through dispatch()", stringify!($name)))
    }};
}

macro_rules! is_async_arm {
    (bare_async) => { true };
    (ctx_async) => { true };
    ($other:ident) => { false };
}

macro_rules! dispatch_table {
    ($(
        $(#[doc = $doc:literal])*
        $kind:ident $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty ;
    )*) => {
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

        /// The command surface as *data*: every name, whether it must be
        /// awaited, its arguments, and what it puts on the wire.
        ///
        /// This is the half of the contract that a generator can read.
        /// `command_names` gives the names and nothing else, which is enough
        /// for the dev portal's drift banner and not enough to emit a typed
        /// TypeScript client — that needs the arguments and the return type
        /// too, which until now existed only in `core`'s function signatures
        /// where nothing outside Rust could reach them.
        ///
        /// **`returns` is the success type, not the `Result`.** The error half
        /// is the transport's business: every command that can fail returns
        /// `Result<T, String>`, `dispatch` unwraps it with `?`, and only `T`
        /// is ever serialized. A client sees `T` or an RPC error, never a
        /// serialized `Result`.
        ///
        /// The strings come from `stringify!`, which preserves the source text
        /// — `Vec<crate::db::RecordingRow>` arrives exactly as the table spells
        /// it, not re-printed from the token tree. So the table's spelling *is*
        /// the contract's spelling, which is why every type in it is written as
        /// an absolute `crate::` path: a generator reading this has no module
        /// context to resolve `super::` or a bare `DiskUsage` against.
        ///
        /// Nothing calls this yet — `gen-contract` (WS2.5) is its first real
        /// consumer, and the tests below are the only one today. Clippy runs
        /// without `--all-targets`, so in a shipped build it is genuinely dead
        /// code and `-D warnings` would fail on it. Same reason as
        /// `command_names` above, different condition: that one is reachable
        /// under `devtools`, this one is reachable from nowhere yet.
        #[cfg_attr(not(test), allow(dead_code))]
        pub fn contract_manifest() -> &'static [CommandSpec] {
            &[
                $(
                    CommandSpec {
                        name: stringify!($name),
                        is_async: is_async_arm!($kind),
                        args: &[
                            $( CommandArg { name: stringify!($arg), ty: stringify!($ty) }, )*
                        ],
                        returns: stringify!($ret),
                    },
                )*
            ]
        }

        /// The same table again, with every type resolved through ts-rs
        /// instead of `stringify!`.
        ///
        /// This is what closes the gap `contract::types` documents: the
        /// `&'static str` manifest names types as *source text*, and a string
        /// cannot be turned back into a type to ask it for its declaration.
        /// Here the macro still has the real types, so it asks them directly.
        ///
        /// The alternative was a Rust-path-to-TypeScript table inside the
        /// generator, parsing `Vec<..>`, `HashMap<..>` and `()` by hand. That
        /// would be a third list able to disagree with the other two, in the
        /// one workstream whose purpose is deleting exactly that. ts-rs already
        /// knows how every one of these renders, including the generics.
        ///
        /// Every type in the table must therefore implement `TS`. That is a
        /// compile error here rather than a runtime surprise in the generator,
        /// which is where it belongs.
        /// What each command does, in one line, taken from the doc comment on
        /// its table row.
        ///
        /// This is where the dev portal's command help comes from since WS2.7.
        /// It used to be a `description` field in `src/dev/registry.ts`, a
        /// second hand-written list; a doc comment is the one place a reader
        /// already looks, and `cargo doc` renders it too.
        ///
        /// Empty for a row with no doc comment, which is a row whose help text
        /// nobody has written rather than an error.
        #[cfg_attr(not(test), allow(dead_code))]
        pub fn command_descriptions() -> &'static [(&'static str, &'static str)] {
            &[ $( (stringify!($name), concat!($($doc),*)), )* ]
        }

        #[cfg_attr(not(test), allow(dead_code))]
        pub fn contract_manifest_ts(cfg: &ts_rs::Config) -> Vec<CommandTsSpec> {
            vec![
                $(
                    CommandTsSpec {
                        name: stringify!($name),
                        is_async: is_async_arm!($kind),
                        args: vec![
                            $( CommandTsArg {
                                name: to_camel(stringify!($arg)),
                                ts: <$ty as ts_rs::TS>::name(cfg),
                            }, )*
                        ],
                        returns: <$ret as ts_rs::TS>::name(cfg),
                    },
                )*
            ]
        }

        /// Whether a command has to be awaited rather than run on a blocking
        /// thread. Callers need this to put each command on the right kind of
        /// thread; see `dispatch_blocking`.
        pub fn is_async_command(command: &str) -> bool {
            match command {
                $( stringify!($name) => is_async_arm!($kind), )*
                // The portal's own commands answer for themselves, and only in
                // a build that has them.
                #[cfg(feature = "devtools")]
                other if other.starts_with("dev_") => crate::dev::is_async_dev_command(other),
                _ => false,
            }
        }

        /// Runs one command by name. `args` is the frontend's argument object;
        /// `null` and `{}` are both accepted for a command that takes none.
        ///
        /// Async because three commands are. Everything else here is *blocking*
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
                        invoke_one!($kind $name, $ret, ctx, parsed, $($arg,)*)
                    }
                )*
                // Delegated rather than tabled here, because the portal's surface
                // is behind a feature and must not exist at all in a shipped
                // build. `dev::dispatch` is that whole module (WS3.7).
                #[cfg(feature = "devtools")]
                other if other.starts_with("dev_") => {
                    crate::dev::dispatch_dev(ctx, other, args).await
                }
                other => Err(format!("unknown command: {other}")),
            }
        }

        /// The synchronous half, for running on a thread that is allowed to
        /// block. Identical to `dispatch` except that the async commands
        /// refuse rather than pretending.
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
                        invoke_one_blocking!($kind $name, $ret, ctx, parsed, $($arg,)*)
                    }
                )*
                #[cfg(feature = "devtools")]
                other if other.starts_with("dev_") => {
                    crate::dev::dispatch_dev_blocking(ctx, other, args)
                }
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
    /// Starts the active capture backend, after a free-space preflight. Races the state machine's own automatic start, because the supervisor does not know about this call.
    ctx_result  start_recording() -> ();
    /// Stops capture and returns the path of the file produced.
    ctx_result  stop_recording() -> String;
    /// Whether the backend believes it is capturing right now.
    ctx_result  is_recording() -> bool;
    /// Every row in the VOD library, newest first.
    ctx_result  list_recordings() -> Vec<crate::db::RecordingRow>;
    /// Reconciles rows against the folder: drops rows whose file is gone, imports untracked .mp4/.mkv files. Deletes rows.
    ctx_result  rescan_recordings() -> crate::db::reconcile::ReconcileReport;
    /// Timeline markers for one recording, ordered by video time.
    ctx_result  get_recording_markers(recording_id: i64) -> Vec<crate::db::MarkerRow>;
    /// Advantage-curve samples for one recording. An empty array means the recording predates sampling, not an error.
    ctx_result  get_recording_samples(recording_id: i64) -> Vec<crate::db::SampleRow>;
    /// Total library bytes, recording count, and free space on the recordings volume.
    ctx_result  get_disk_usage() -> crate::core::DiskUsage;
    /// The saved policy. null on either field means that dimension is unbounded.
    ctx_result  get_retention_policy() -> crate::db::RetentionPolicy;
    /// Saves the policy AND immediately enforces it, which deletes files. Use dev_retention_preview first.
    ctx_result  set_retention_policy(policy: crate::db::RetentionPolicy) -> crate::retention::EnforcementReport;
    /// Pins or unpins a recording. Pinned rows are exempt from retention deletion.
    ctx_result  set_pinned(recording_id: i64, pinned: bool) -> ();
    /// Dry run of enforcement under the given policy. Writes nothing; the safe counterpart to set_retention_policy.
    ctx_result  preview_retention_policy(policy: crate::db::RetentionPolicy) -> crate::retention::EnforcementReport;
    /// Deletes one recording's row and its file on disk.
    ctx_result  delete_recording(recording_id: i64) -> ();
    /// Absolute path of the recordings directory.
    ctx_plain   get_recordings_dir() -> String;
    /// Every key/value in the settings_kv store (theme, default sort, …).
    ctx_result  get_ui_prefs() -> std::collections::HashMap<String, String>;
    /// Writes one UI preference. Unseeded store: a missing key means 'use the frontend default'.
    ctx_result  set_ui_pref(key: String, value: String) -> ();
    /// Whether the app is registered to start on login, read live from the platform (HKCU\...\Run on Windows) rather than from settings_kv. `supported: false` means this build has no autostart control.
    ctx_result  get_autostart() -> crate::core::AutostartStatus;
    /// Adds or removes the login entry for this executable, then returns what the platform says afterwards, which is not always what was asked for. Writes outside the app's own data: enabling here really does register the running binary, dev build included.
    ctx_result  set_autostart(enabled: bool) -> crate::core::AutostartStatus;
    /// The audio capture preset. Unlike the settings_kv prefs, this is parsed and validated backend-side: it decides what gets recorded.
    ctx_result  get_audio_preset() -> crate::recorder::audio::AudioPreset;
    /// Chooses what gets captured and how it is split across mp4 audio tracks. Track 0 is always the combined mix.
    ctx_result  set_audio_preset(preset: crate::recorder::audio::AudioPreset) -> ();
    /// The capture_backend setting (libobs or own), the backend actually live, and which backends this build can construct, each with the reason when it cannot. Refuses in a process that does not own the recorder.
    ctx_result  get_capture_backend() -> crate::recorder::backend::CaptureBackendStatus;
    /// Saves which capture backend the daemon builds and puts it in place for the next recording. Refuses a backend this build cannot construct, and refuses while a game is in progress: the backend is never swapped mid-recording.
    ctx_result  set_capture_backend(backend: crate::recorder::backend::CaptureBackend) -> crate::recorder::backend::CaptureBackendStatus;
    /// Audio input devices for the microphone picker, default first. Empty off Windows.
    bare_result list_audio_inputs() -> Vec<crate::recorder::audio::AudioInputDevice>;
    /// Extracts one audio stem to a cached sidecar so the review player can play it. Rejects track 0, which plays from the video itself.
    ctx_result  extract_audio_track(recording_path: String, track_index: usize) -> String;
    /// One-shot LCU check: lockfile discovery, auth, gameflow phase, summoner. Infallible; failures come back in the `error` field.
    bare_async  lcu_status() -> crate::core::LcuStatus;
    /// Matches every recording with no champion or result against the client's match history, by when it was played. Refuses a recording that overlaps more than one game rather than guessing. Needs the League Client running.
    ctx_async   backfill_match_metadata() -> crate::backfill::BackfillReport;
    /// Cached Data Dragon art for a page of rows: champions by display name, items and runes by id, spells by display name. Fetches whatever is not cached yet. Anything that could not be resolved is absent from the result rather than null.
    ctx_async   resolve_icons(request: crate::ddragon::IconRequest) -> crate::ddragon::IconSet;
    /// Current supervisor state and the last finalized recording.
    ctx_plain   game_state_status() -> crate::state_machine::SupervisorStatus;
    /// What the last background check found, with installability recomputed against live state. `unsupported` in a devtools build, this one included, because the update seam is never wired there.
    ctx_result  get_update_status() -> crate::update::UpdateStatus;
    /// Asks for a check now rather than waiting for the six-hourly one. Returns as soon as the request is handed over; the answer arrives on the `update-status-changed` event. Refuses in a devtools build.
    ctx_result  check_for_update() -> ();
    /// Downloads the offered installer and hands the machine over to it, which ends the process. Refuses while anything is being recorded, and refuses outright in a devtools build.
    ctx_result  install_update() -> ();
    /// Stops the recorder itself, so nothing records in the background afterwards. Answers `recordingInFlight` instead of stopping when a game is being recorded and `force` is false; call again with `force` once the person has agreed.
    ctx_result  quit_recorder(force: bool) -> crate::core::QuitOutcome;
    /// The review's game for a recording, made from the recording if it has none: every recording from before VOD review, and anything reconcile imported. A game made this way has no objective snapshot.
    ctx_result  open_game_for_recording(recording_id: i64) -> i64;
    /// Everything the review form shows for one game: its header, the saved review (null until the first save), the death-marker count, the objectives it was played against, and its takeaways. null if there is no such game.
    ctx_result  get_game_review(game_id: i64) -> Option<crate::db::review::GameReview>;
    /// Saves the whole review for a game, replacing what was there. A null field is cleared, and a null deaths means 'use the death markers'. Refuses a negative count.
    ctx_result  save_game_review(game_id: i64, review: crate::db::review::ReviewInput) -> ();
    /// Ticks or unticks an objective for a game. Refuses one the game was not played against.
    ctx_result  set_objective_ticked(game_id: i64, objective_id: i64, ticked: bool) -> ();
    /// Objectives, newest first. null lists every status.
    ctx_result  list_objectives(status: Option<crate::db::review::ObjectiveStatus>) -> Vec<crate::db::review::Objective>;
    /// Adds an active objective. Games that start from now on are played against it. Refuses an empty body.
    ctx_result  create_objective(body: String, category: crate::db::review::ObjectiveCategory) -> crate::db::review::Objective;
    /// Rewrites an objective's text and category. Games already played against it see the new text.
    ctx_result  update_objective(objective_id: i64, body: String, category: crate::db::review::ObjectiveCategory) -> crate::db::review::Objective;
    /// Activates, pauses or retires an objective. Only active objectives are snapshotted into new games; past games keep theirs.
    ctx_result  set_objective_status(objective_id: i64, status: crate::db::review::ObjectiveStatus) -> crate::db::review::Objective;
    /// Adds a takeaway to a game or a block. Refuses an empty body.
    ctx_result  add_takeaway(owner: crate::db::review::TakeawayOwner, body: String) -> crate::db::review::Takeaway;
    /// Deletes a takeaway. An objective it was promoted to stays.
    ctx_result  delete_takeaway(takeaway_id: i64) -> ();
    /// Makes an active objective from a takeaway, in one transaction. A takeaway already promoted returns the objective it became rather than making a second.
    ctx_result  promote_takeaway(takeaway_id: i64, category: crate::db::review::ObjectiveCategory) -> crate::db::review::Objective;
    /// Starts a new block at a game: it and every later game in its block move to the new one. Returns the new block's id. Refuses the first game of a block.
    ctx_result  split_block(game_id: i64) -> i64;
    /// Moves every game and takeaway from one block into another and deletes the emptied block.
    ctx_result  merge_blocks(into_block_id: i64, from_block_id: i64) -> ();
    /// Imports rows of the review spreadsheet, as the Objectives view parsed them from a CSV, in one transaction. Safe to run twice: rows match existing games within five minutes, and only empty fields are filled.
    ctx_result  import_review_rows(rows: Vec<crate::db::review_import::ImportRow>) -> crate::db::review_import::ImportReport;
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
        let db = Arc::new(Db::open_temporary().unwrap());
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-dispatch-test-{}",
            std::process::id()
        ));
        let supervisor =
            state_machine::Supervisor::new(Arc::clone(&recorder), dir.clone(), Arc::clone(&db));
        Ctx::new(recorder, supervisor, db, dir.clone(), dir.join("ddragon"), None)
    }

    /// Quitting is the one command whose whole job is to end the process, so
    /// what is worth pinning is that it refuses to in every case but the one.
    mod quitting {
        use super::*;
        use crate::core::{QuitOutcome, quit_recorder};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// A recorder that says it is mid-game and nothing else.
        ///
        /// `StubRecorder` would have to be driven through a real `start` to
        /// report this, which means a fixture on disk and a config, none of
        /// which is what these tests are about. What `quit_recorder` reads is
        /// one boolean, so that is what this supplies.
        struct Busy;

        impl crate::recorder::Recorder for Busy {
            fn start(
                &mut self,
                _config: crate::recorder::RecordConfig,
            ) -> Result<(), crate::recorder::RecorderError> {
                unreachable!("nothing in these tests starts a recording")
            }

            fn stop(
                &mut self,
            ) -> Result<crate::recorder::RecordingOutput, crate::recorder::RecorderError> {
                unreachable!("nor stops one")
            }

            fn is_recording(&self) -> bool {
                true
            }

            fn backend_name(&self) -> String {
                "busy".to_string()
            }
        }

        /// A `Ctx` whose quit seam counts calls instead of stopping anything.
        fn ctx_counting(asks: &Arc<AtomicUsize>) -> Ctx {
            let mut ctx = ctx();
            let asks = Arc::clone(asks);
            ctx.set_quit_requester(Box::new(move || {
                asks.fetch_add(1, Ordering::SeqCst);
            }));
            ctx
        }

        /// The UI forwards every command to the daemon, so this only ever runs
        /// where the seam is set. A process that answered it anyway would be
        /// claiming to have stopped a recorder living somewhere else.
        #[test]
        fn a_process_that_does_not_own_the_recorder_refuses() {
            let error = quit_recorder(&ctx(), true).expect_err("no seam, no shutdown");
            assert!(error.contains("does not own the recorder"), "{error}");
        }

        #[test]
        fn an_idle_recorder_is_stopped_without_asking() {
            let asks = Arc::new(AtomicUsize::new(0));
            let ctx = ctx_counting(&asks);

            assert_eq!(quit_recorder(&ctx, false), Ok(QuitOutcome::ShuttingDown));
            assert_eq!(asks.load(Ordering::SeqCst), 1);
        }

        /// `force` is the answer to the question the refusal asks, so the same
        /// call with it set has to go through rather than refuse twice.
        #[test]
        fn force_stops_it_regardless() {
            let asks = Arc::new(AtomicUsize::new(0));
            let ctx = ctx_counting(&asks);

            assert_eq!(quit_recorder(&ctx, true), Ok(QuitOutcome::ShuttingDown));
            assert_eq!(asks.load(Ordering::SeqCst), 1);
        }

        /// **The refusal must not be a half-quit.** Returning
        /// `RecordingInFlight` while having already asked the daemon to stop
        /// would lose the game the refusal exists to protect, and the caller
        /// would have no way to tell.
        #[test]
        fn a_refusal_stops_nothing() {
            let asks = Arc::new(AtomicUsize::new(0));
            let mut ctx = ctx();
            let counter = Arc::clone(&asks);
            ctx.set_quit_requester(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));
            *ctx.recorder.lock().unwrap() = Box::new(Busy);

            assert_eq!(quit_recorder(&ctx, false), Ok(QuitOutcome::RecordingInFlight));
            assert_eq!(asks.load(Ordering::SeqCst), 0, "a refusal must not have stopped anything");
        }
    }

    /// The `capture_backend` switch (WS1.7). What is worth pinning is the
    /// refusals, because each one guards against a recording that would be
    /// lost or fabricated, and that the swap reaches the recorder the
    /// supervisor shares.
    mod capture_backend {
        use super::*;
        use crate::core::{get_capture_backend, set_capture_backend};
        use crate::recorder::backend::{Backends, CaptureBackend, CaptureBackendOption};
        use crate::recorder::{RecordConfig, RecorderError, RecordingOutput};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Why the own backend cannot be built, in the fake below: the
        /// shape of a refusal below its OS floor.
        const OWN_UNAVAILABLE: &str = "the own capture backend needs Windows build 20348 or newer";

        /// A recorder that is only a name, and optionally mid-game.
        struct Named(&'static str, bool);

        impl Recorder for Named {
            fn start(&mut self, _config: RecordConfig) -> Result<(), RecorderError> {
                unreachable!("nothing here records")
            }
            fn stop(&mut self) -> Result<RecordingOutput, RecorderError> {
                unreachable!("nor stops")
            }
            fn is_recording(&self) -> bool {
                self.1
            }
            fn backend_name(&self) -> String {
                self.0.to_string()
            }
        }

        /// A build offering libobs, and the own backend only if `own_built`.
        /// Counts constructions so a no-op can be told from a rebuild.
        struct Fake {
            own_built: bool,
            builds: Arc<AtomicUsize>,
        }

        impl Backends for Fake {
            fn options(&self) -> Vec<CaptureBackendOption> {
                vec![
                    CaptureBackendOption { backend: CaptureBackend::Libobs, unavailable: None },
                    CaptureBackendOption {
                        backend: CaptureBackend::Own,
                        unavailable: (!self.own_built).then(|| OWN_UNAVAILABLE.to_string()),
                    },
                ]
            }
            fn build(&self, backend: CaptureBackend) -> Box<dyn Recorder> {
                self.builds.fetch_add(1, Ordering::SeqCst);
                Box::new(Named(backend.as_pref(), false))
            }
        }

        /// A user who saved libobs, which is the case every refusal below is
        /// about: a switch *away* from the backend in use. The default is
        /// `own` since #243, and `an_unset_key_reports_own` covers that.
        fn ctx_with(own_built: bool) -> (Ctx, Arc<AtomicUsize>) {
            let builds = Arc::new(AtomicUsize::new(0));
            let mut ctx = ctx();
            ctx.db.set_capture_backend(CaptureBackend::Libobs).unwrap();
            ctx.set_backends(Box::new(Fake { own_built, builds: Arc::clone(&builds) }));
            (ctx, builds)
        }

        #[test]
        fn an_unset_key_reports_own() {
            let mut ctx = ctx();
            ctx.set_backends(Box::new(Fake { own_built: true, builds: Arc::default() }));
            assert_eq!(get_capture_backend(&ctx).unwrap().configured, CaptureBackend::Own);
        }

        /// The UI forwards both commands to the daemon, so this only runs where
        /// the seam is set; anywhere else, answering would describe a recorder
        /// that is not there.
        #[test]
        fn a_process_that_does_not_own_the_recorder_refuses() {
            let err = get_capture_backend(&ctx()).expect_err("no seam");
            assert!(err.contains("does not own the recorder"), "{err}");
            let err = set_capture_backend(&ctx(), CaptureBackend::Libobs).expect_err("no seam");
            assert!(err.contains("does not own the recorder"), "{err}");
        }

        #[test]
        fn the_status_reports_the_setting_the_live_backend_and_every_option() {
            let (ctx, _) = ctx_with(false);
            let status = get_capture_backend(&ctx).unwrap();
            assert_eq!(status.configured, CaptureBackend::Libobs);
            assert_eq!(status.active, "stub");
            assert_eq!(status.options.len(), 2);
            assert_eq!(
                status.options[1].unavailable.as_deref(),
                Some(OWN_UNAVAILABLE),
                "the unbuilt backend is listed, with its reason, not left out"
            );
        }

        /// **The whole feature's safety property**: an unbuildable
        /// own backend cannot be chosen, and trying writes nothing.
        #[test]
        fn an_unbuilt_backend_cannot_be_chosen() {
            let (ctx, builds) = ctx_with(false);
            let err = set_capture_backend(&ctx, CaptureBackend::Own).expect_err("unbuilt");
            assert!(err.contains(OWN_UNAVAILABLE), "{err}");
            assert_eq!(ctx.db.get_capture_backend().unwrap(), CaptureBackend::Libobs);
            assert_eq!(builds.load(Ordering::SeqCst), 0);
            assert_eq!(ctx.recorder.lock().unwrap().backend_name(), "stub");
        }

        #[test]
        fn a_change_replaces_the_recorder_the_supervisor_shares() {
            let (ctx, builds) = ctx_with(true);
            let status = set_capture_backend(&ctx, CaptureBackend::Own).unwrap();

            assert_eq!(status.configured, CaptureBackend::Own);
            assert_eq!(status.active, "own");
            assert_eq!(ctx.db.get_capture_backend().unwrap(), CaptureBackend::Own);
            assert_eq!(builds.load(Ordering::SeqCst), 1);
            // The same `Arc` the supervisor was built with, so the next game
            // it starts is on the new backend.
            assert_eq!(ctx.recorder.lock().unwrap().backend_name(), "own");
        }

        #[test]
        fn choosing_the_current_backend_rebuilds_nothing() {
            let (ctx, builds) = ctx_with(true);
            set_capture_backend(&ctx, CaptureBackend::Libobs).unwrap();
            assert_eq!(builds.load(Ordering::SeqCst), 0);
            assert_eq!(ctx.recorder.lock().unwrap().backend_name(), "stub");
        }

        /// Never a swap mid-recording: refused, nothing saved, and the live
        /// recorder is the one that was recording.
        #[test]
        fn a_recording_in_flight_is_never_swapped() {
            let (ctx, builds) = ctx_with(true);
            *ctx.recorder.lock().unwrap() = Box::new(Named("busy", true));

            let err = set_capture_backend(&ctx, CaptureBackend::Own).expect_err("recording");
            assert!(err.contains("recording is in progress"), "{err}");
            assert_eq!(ctx.db.get_capture_backend().unwrap(), CaptureBackend::Libobs);
            assert_eq!(builds.load(Ordering::SeqCst), 0);
            assert_eq!(ctx.recorder.lock().unwrap().backend_name(), "busy");
        }

        /// `every_command_round_trips` reaches `set_capture_backend` without a
        /// seam, so it only proves the argument parses. This proves the wire
        /// spelling reaches the swap.
        #[tokio::test]
        async fn the_wire_spelling_reaches_the_swap() {
            let (ctx, _) = ctx_with(true);
            let out = dispatch(&ctx, "set_capture_backend", json!({ "backend": "own" }))
                .await
                .unwrap();
            assert_eq!(out["configured"], "own");
            assert_eq!(out["active"], "own");
            assert_eq!(out["options"][1]["unavailable"], Value::Null);
        }
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
            // Safe to run for real: the test `Ctx` has no autostart control,
            // so this is refused before it can reach a registry.
            "set_autostart" => json!({ "enabled": true }),
            // Internally tagged on `preset`, so the value is an object.
            "set_audio_preset" => json!({ "preset": { "preset": "game" } }),
            // Refused here: the test `Ctx` has no backends seam, so this
            // exercises the argument mapping and never replaces a recorder.
            // `capture_backend` below drives it with one installed.
            "set_capture_backend" => json!({ "backend": "libobs" }),
            "extract_audio_track" => json!({ "recordingPath": "/tmp/nope.mp4", "trackIndex": 1 }),
            // Every list empty, so `resolve_icons` has nothing to look
            // up and the suite never reaches a CDN — this exercises the
            // argument mapping and nothing else.
            "resolve_icons" => json!({ "request": {} }),
            // `false`, so the round trip exercises the argument and not the
            // shutdown: the test `Ctx` leaves the quit seam unset, so this
            // refuses before it can reach anything, which is the same reason
            // the update commands are safe to drive here.
            "quit_recorder" => json!({ "force": false }),
            // The review commands mostly refuse here, on ids that do not
            // exist; what the round trip checks is that they parsed.
            "open_game_for_recording" => json!({ "recordingId": 1 }),
            "get_game_review" | "split_block" => json!({ "gameId": 1 }),
            "save_game_review" => json!({
                "gameId": 1,
                "review": {
                    "game_rating": "win", "lane_rating": "neutral", "mental_rating": "good",
                    "first_clear_ms": 178000, "smites_at_clear": 1, "deaths": null,
                    "free_notes": ""
                }
            }),
            "set_objective_ticked" => json!({ "gameId": 1, "objectiveId": 1, "ticked": true }),
            "list_objectives" => json!({ "status": "active" }),
            "create_objective" => json!({ "body": "ward at 2:45", "category": "macro" }),
            "update_objective" => json!({ "objectiveId": 1, "body": "b", "category": "lane" }),
            "set_objective_status" => json!({ "objectiveId": 1, "status": "retired" }),
            "add_takeaway" => json!({ "owner": { "kind": "game", "id": 1 }, "body": "t" }),
            "delete_takeaway" => json!({ "takeawayId": 1 }),
            "promote_takeaway" => json!({ "takeawayId": 1, "category": "other" }),
            "merge_blocks" => json!({ "intoBlockId": 1, "fromBlockId": 2 }),
            "import_review_rows" => json!({ "rows": [{
                "line": 2, "started_at": 0, "block": "1", "champion": "Lee Sin", "matchup": "Vi",
                "game": "win", "lane": "neutral", "mental": "good", "clear_ms": 178000,
                "smites": 1, "deaths": 3, "objectives": ["ward"], "takeaways": [],
                "block_takeaways": []
            }] }),
            // The three update commands take no arguments and reach no
            // network here: the test `Ctx` leaves the update seam unset, so
            // `check_for_update` and `install_update` both refuse with "not
            // available in this build" and `get_update_status` reports
            // `Unsupported`. That is deliberate — an `install_update` that
            // worked under `cargo test` would restart the test binary into
            // an installer.
            _ => json!({}),
        }
    }

    /// The manifest and the dispatcher come from the same macro invocation, so
    /// they cannot disagree about *which* commands exist. This pins that they
    /// do not, which is what makes the manifest usable as a source of truth
    /// rather than a second list to keep in step.
    #[test]
    fn the_manifest_describes_exactly_the_commands_that_dispatch() {
        let manifest: Vec<&str> = contract_manifest().iter().map(|c| c.name).collect();
        assert_eq!(
            manifest,
            command_names(),
            "the manifest and command_names come from the same rows and must agree"
        );
    }

    /// `is_async` decides which of the two dispatchers a caller may use, and
    /// getting it wrong means either blocking an async worker for the length of
    /// an ffmpeg run or refusing a command that would have worked.
    #[test]
    fn the_manifest_agrees_with_is_async_command() {
        for spec in contract_manifest() {
            assert_eq!(
                spec.is_async,
                is_async_command(spec.name),
                "{} disagrees about being async",
                spec.name
            );
        }
    }

    /// Every command has to declare something it puts on the wire. The compiler
    /// already proves the declared type *matches the function* — a wrong one is
    /// an `E0308` at the `?`, not a silent lie — so what is left to check is
    /// that nothing is blank.
    #[test]
    fn every_command_declares_a_return_type() {
        for spec in contract_manifest() {
            assert!(
                !spec.returns.trim().is_empty(),
                "{} declares no return type",
                spec.name
            );
        }
    }

    /// The generator emits one TypeScript method per command with named
    /// arguments, so a duplicate name or a duplicate argument within a command
    /// would emit something that does not compile — better to fail here.
    #[test]
    fn names_are_unique_within_the_surface_and_within_each_command() {
        let mut seen = std::collections::HashSet::new();
        for spec in contract_manifest() {
            assert!(seen.insert(spec.name), "duplicate command: {}", spec.name);
            let mut args = std::collections::HashSet::new();
            for arg in spec.args {
                assert!(
                    args.insert(arg.name),
                    "{} has two arguments called {}",
                    spec.name,
                    arg.name
                );
                assert!(
                    !arg.ty.trim().is_empty(),
                    "{}({}) has no type",
                    spec.name,
                    arg.name
                );
            }
        }
    }

    /// The arguments in the manifest are the ones `dispatch` actually
    /// deserializes, because both come from the same row. Pinned against the
    /// round-trip fixtures so that a row gaining an argument without the
    /// fixture gaining one fails here rather than at runtime.
    #[test]
    fn manifest_arguments_match_the_round_trip_fixtures() {
        for spec in contract_manifest() {
            let fixture = sample_args(spec.name);
            let obj = fixture.as_object().expect("fixtures are objects");
            for arg in spec.args {
                let camel = to_camel(arg.name);
                assert!(
                    obj.contains_key(&camel),
                    "{} takes {} ({}), which the round-trip fixture does not send",
                    spec.name,
                    arg.name,
                    camel
                );
            }
        }
    }

    /// The whole point of the table: every command must be reachable by name
    /// with the arguments the frontend really sends, and must not fail
    /// *argument parsing*. A command is free to return a business error, a
    /// missing recording or no ffmpeg, but "bad arguments for …" means the
    /// camelCase mapping or a type is wrong, which is exactly the class of bug
    /// the Tauri macro used to catch at compile time.
    ///
    /// **WS2.7 kept this deliberately.** The task deletes the things that
    /// existed because two hand-written lists could disagree, and
    /// `gen-contract --check` replaces those. It does not replace this: the
    /// generator proves the emitted TypeScript matches the declaration, which
    /// is a statement about two files, while this proves `dispatch` can
    /// actually parse what that client sends, which is a statement about
    /// runtime. Deleting it would trade a guarded failure for an unguarded
    /// one.
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

    /// WS9's review, driven the way the form drives it: over the wire, with
    /// the argument and value spellings the generated client sends.
    #[tokio::test]
    async fn a_review_round_trips_through_dispatch() {
        let ctx = ctx();
        let objective = dispatch(&ctx, "create_objective", json!({ "body": "ward", "category": "macro" }))
            .await
            .unwrap();
        assert_eq!(objective["status"], "active");
        let recording = ctx.db.begin_recording("C:/vods/a.mp4", 1000).unwrap();
        ctx.db.start_game(Some(recording), 1000).unwrap();

        let game = dispatch(&ctx, "open_game_for_recording", json!({ "recordingId": recording }))
            .await
            .unwrap();
        dispatch(&ctx, "save_game_review", json!({
            "gameId": game,
            "review": { "game_rating": "loss", "lane_rating": null, "mental_rating": "bad",
                        "first_clear_ms": null, "smites_at_clear": null, "deaths": 4,
                        "free_notes": "tilted" }
        }))
        .await
        .unwrap();
        dispatch(&ctx, "set_objective_ticked",
            json!({ "gameId": game, "objectiveId": objective["id"], "ticked": true }))
            .await
            .unwrap();
        let takeaway = dispatch(&ctx, "add_takeaway",
            json!({ "owner": { "kind": "game", "id": game }, "body": "contest grubs" }))
            .await
            .unwrap();
        let promoted = dispatch(&ctx, "promote_takeaway",
            json!({ "takeawayId": takeaway["id"], "category": "macro" }))
            .await
            .unwrap();

        let review = dispatch(&ctx, "get_game_review", json!({ "gameId": game })).await.unwrap();
        assert_eq!(review["review"]["game_rating"], "loss");
        assert_eq!(review["review"]["deaths"], 4);
        assert_eq!(review["objectives"][0]["ticked"], true);
        assert_eq!(review["takeaways"][0]["promoted_to_id"], promoted["id"]);
        let active = dispatch(&ctx, "list_objectives", json!({ "status": "active" })).await.unwrap();
        assert_eq!(active.as_array().unwrap().len(), 2);

        let missing = dispatch(&ctx, "get_game_review", json!({ "gameId": 999 })).await.unwrap();
        assert!(missing.is_null());
        let refused = dispatch(&ctx, "create_objective", json!({ "body": " ", "category": "other" }))
            .await
            .unwrap_err();
        assert_eq!(refused, "an objective cannot be empty");
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

    /// Stands in for the registry. `sticky` models the case that actually
    /// matters on Windows: a write that reports success and changes nothing,
    /// because policy or permissions overruled it.
    struct FakeAutostart {
        enabled: Mutex<bool>,
        sticky: bool,
    }

    impl FakeAutostart {
        fn new(enabled: bool) -> Self {
            Self {
                enabled: Mutex::new(enabled),
                sticky: false,
            }
        }

        fn ignoring_writes() -> Self {
            Self {
                enabled: Mutex::new(false),
                sticky: true,
            }
        }
    }

    impl super::super::Autostart for FakeAutostart {
        fn is_enabled(&self) -> Result<bool, String> {
            Ok(*self.enabled.lock().unwrap())
        }

        fn enable(&self) -> Result<(), String> {
            if !self.sticky {
                *self.enabled.lock().unwrap() = true;
            }
            Ok(())
        }

        fn disable(&self) -> Result<(), String> {
            if !self.sticky {
                *self.enabled.lock().unwrap() = false;
            }
            Ok(())
        }
    }

    fn ctx_with_autostart(autostart: FakeAutostart) -> Ctx {
        let mut ctx = ctx();
        ctx.set_autostart(Box::new(autostart));
        ctx
    }

    #[tokio::test]
    async fn autostart_can_be_turned_on_and_off_and_reads_back() {
        let ctx = ctx_with_autostart(FakeAutostart::new(false));

        let status = dispatch(&ctx, "get_autostart", Value::Null).await.unwrap();
        assert_eq!(status["enabled"], json!(false));
        assert_eq!(status["supported"], json!(true));

        let status = dispatch(&ctx, "set_autostart", json!({ "enabled": true }))
            .await
            .unwrap();
        assert_eq!(status["enabled"], json!(true), "set should report the new state");
        let status = dispatch(&ctx, "get_autostart", Value::Null).await.unwrap();
        assert_eq!(status["enabled"], json!(true), "the change should persist");

        dispatch(&ctx, "set_autostart", json!({ "enabled": false }))
            .await
            .unwrap();
        let status = dispatch(&ctx, "get_autostart", Value::Null).await.unwrap();
        assert_eq!(status["enabled"], json!(false));
    }

    /// The whole reason `set_autostart` re-reads instead of echoing its
    /// argument: a `Run` entry write can be overruled, and a checkbox that
    /// ticked anyway would promise a login start that never happens.
    #[tokio::test]
    async fn autostart_reports_the_platform_not_the_request() {
        let ctx = ctx_with_autostart(FakeAutostart::ignoring_writes());
        let status = dispatch(&ctx, "set_autostart", json!({ "enabled": true }))
            .await
            .unwrap();
        assert_eq!(
            status["enabled"],
            json!(false),
            "a write the platform ignored must not report itself as enabled"
        );
    }

    /// `Ctx::new` leaves autostart unset, so nothing in `cargo test` — this
    /// module's `every_command_round_trips` included — can write a real login
    /// entry on the machine running the suite.
    #[tokio::test]
    async fn without_a_control_autostart_reports_unsupported_and_refuses_writes() {
        let ctx = ctx();
        let status = dispatch(&ctx, "get_autostart", Value::Null).await.unwrap();
        assert_eq!(status["supported"], json!(false));
        assert_eq!(status["enabled"], json!(false));

        let err = dispatch(&ctx, "set_autostart", json!({ "enabled": true }))
            .await
            .expect_err("a Ctx with no autostart control must not silently succeed");
        assert!(err.contains("not available"), "{err}");
    }

    #[test]
    fn notification_defaults_are_quiet_where_it_matters() {
        use super::super::{NotificationPrefs, NotifyKind};
        let d = NotificationPrefs::default();
        // Finishing and failing are the two the user cannot otherwise learn
        // about with the window closed.
        assert!(d.allows(NotifyKind::RecordingFinished));
        assert!(d.allows(NotifyKind::RecordingFailed));
        // Starting is not: the user is about to be in a game, and a popup
        // over it is worse than useless.
        assert!(!d.allows(NotifyKind::RecordingStarted));
    }

    #[test]
    fn the_master_switch_silences_everything_including_the_one_time_notice() {
        use super::super::{NotificationPrefs, NotifyKind, NOTIFY_MASTER_KEY};
        let mut prefs = std::collections::HashMap::new();
        prefs.insert(NOTIFY_MASTER_KEY.to_string(), "off".to_string());
        let p = NotificationPrefs::from_prefs(&prefs);
        for kind in [
            NotifyKind::RecordingStarted,
            NotifyKind::RecordingFinished,
            NotifyKind::RecordingFailed,
            NotifyKind::CloseToTray,
        ] {
            assert!(!p.allows(kind), "{kind:?} should be silenced");
        }
    }

    #[test]
    fn an_unrecognised_notification_value_keeps_the_default() {
        use super::super::{NotificationPrefs, NotifyKind, NOTIFY_FINISHED_KEY};
        let mut prefs = std::collections::HashMap::new();
        prefs.insert(NOTIFY_FINISHED_KEY.to_string(), "maybe".to_string());
        assert!(NotificationPrefs::from_prefs(&prefs).allows(NotifyKind::RecordingFinished));
    }

    #[test]
    fn individual_kinds_can_be_turned_on_and_off() {
        use super::super::{NotificationPrefs, NotifyKind, NOTIFY_FINISHED_KEY, NOTIFY_STARTED_KEY};
        let mut prefs = std::collections::HashMap::new();
        prefs.insert(NOTIFY_STARTED_KEY.to_string(), "on".to_string());
        prefs.insert(NOTIFY_FINISHED_KEY.to_string(), "off".to_string());
        let p = NotificationPrefs::from_prefs(&prefs);
        assert!(p.allows(NotifyKind::RecordingStarted));
        assert!(!p.allows(NotifyKind::RecordingFinished));
    }

    #[tokio::test]
    async fn a_one_time_notice_fires_once_and_can_be_reset() {
        use super::super::{mark_notice_seen, notice_seen, NOTICE_CLOSE_TO_TRAY_KEY};
        let ctx = ctx();
        assert!(!notice_seen(&ctx, NOTICE_CLOSE_TO_TRAY_KEY), "unseen to start");

        mark_notice_seen(&ctx, NOTICE_CLOSE_TO_TRAY_KEY);
        assert!(notice_seen(&ctx, NOTICE_CLOSE_TO_TRAY_KEY), "should not fire twice");

        // "Reset one-time notices" blanks the key. There is no delete
        // command and adding one would mean editing the dispatch table and
        // the dev registry for a button, so an empty value means unseen.
        dispatch(
            &ctx,
            "set_ui_pref",
            json!({ "key": NOTICE_CLOSE_TO_TRAY_KEY, "value": "" }),
        )
        .await
        .unwrap();
        assert!(
            !notice_seen(&ctx, NOTICE_CLOSE_TO_TRAY_KEY),
            "blanking the key must re-arm the notice, or the reset button does nothing"
        );
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

