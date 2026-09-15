//! The one declaration of the `dev_*` command surface. WS2 task 2.7.
//!
//! ## Why this exists
//!
//! `generate_handler!` takes a literal path list: it cannot host a `#[cfg]`
//! attribute, and it cannot host a macro expansion inside its brackets. So the
//! registration list in `lib.rs` was hand-written, and `src/dev/registry.ts`
//! was a second hand-written copy of the same commands on the TypeScript side,
//! carrying the help text and form metadata the portal renders. Two
//! hand-maintained lists is the condition the portal's drift banner existed to
//! make visible.
//!
//! This is the callback-macro shape `contract::types`' boundary list already
//! uses: the table lives here once, and a caller passes a macro that receives
//! it. `lib.rs` hands it one that expands to `generate_handler!`, and
//! `gen-contract` hands it one that emits the portal's spec, so the
//! registration and the portal come from the same tokens and cannot disagree.
//! Issue #74 is the decision that put the `dev_*` commands in the declaration
//! at all.
//!
//! Moving this list found one real drift: `dev_open_portal` was registered in
//! Rust and absent from `registry.ts`. The banner could not have caught it,
//! because it only ever compared the *production* commands.
//!
//! ## What it deliberately does not change
//!
//! Joining the *declaration* is not joining the `rpc` *dispatch*. These stay
//! individually registered behind the `devtools` feature and the portal keeps
//! invoking them through `src/dev/ipc.ts`; moving them onto the pipe is WS3.7.
//! `dev_registered_commands` in particular must stay directly registered,
//! because `devportal.ts` decides whether the portal exists by watching that
//! call reject in a shipped build.
//!
//! ## Shape of a row
//!
//! The doc comment is the help text, which is the whole point: it is where a
//! reader already looks, `cargo doc` renders it, and it cannot fall out of step
//! with a copy in another language. Everything else is what the portal needs to
//! build a form, and every field is spelled out even when empty so the matcher
//! stays simple and a row is readable without consulting the macro.

/// One argument of one dev command, as the portal's form generator sees it.
#[cfg_attr(not(test), allow(dead_code))]
pub struct DevArgSpec {
    pub name: &'static str,
    /// `string`, `number`, `boolean` or `json`, which is what the portal
    /// renders an input for. Spelled rather than derived: these commands have
    /// no `dispatch_table!` row, so there is no Rust type here to ask.
    pub kind: &'static str,
    /// Pre-filled into the form. Empty means no default, which is not the same
    /// as an empty default.
    pub default: &'static str,
    /// Rendered under the input. Empty when nobody has written one.
    pub help: &'static str,
    pub optional: bool,
}

/// One dev command.
#[cfg_attr(not(test), allow(dead_code))]
pub struct DevCommandSpec {
    pub name: &'static str,
    /// The panel heading it sorts under.
    pub group: &'static str,
    /// Writes, deletes, or otherwise cannot simply be re-run. The portal marks
    /// these, which matters in a surface that can wipe the database.
    pub danger: bool,
    /// From the row's doc comment.
    pub description: &'static str,
    /// Whether this command is invoked by name through `rpc`, or is a
    /// `#[tauri::command]` the UI registers directly.
    ///
    /// The portal needs it to know which call to make, and a reader needs it to
    /// know which process answers. It is not written in the table: it is which
    /// of the two tables the row is in, which is a fact that cannot be set
    /// wrong independently of where the command actually lives.
    pub over_rpc: bool,
    pub args: &'static [DevArgSpec],
}

/// The commands that run **in the UI process**, and why each one has to.
///
/// Every one of these reaches something only the process with a window can
/// reach: the desktop shell, a webview, or the question of whether the portal
/// exists at all. They stay `#[tauri::command]`s registered through
/// `generate_handler!`, because there is nothing for the daemon to do with
/// them.
///
/// - `dev_open_portal` creates a window.
/// - `dev_open_data_dir`, `dev_reveal_recording` and `dev_open_fixture` drive
///   the OS file manager. An Explorer window opened by a background daemon can
///   land behind the foreground app, which is the same reason
///   `open_recordings_folder` stayed in `lib.rs`.
/// - `dev_env_info` reports *this* process's paths and build, which is a
///   different answer in each process and is meant to be.
/// - `dev_registered_commands` must stay directly registered, because
///   `devportal.ts` decides whether the portal exists by watching that call
///   reject in a shipped build. Routing it through `rpc` would make it
///   succeed, and the portal would appear in a build that has no portal.
#[macro_export]
macro_rules! dev_ui_command_table {
    ($m:ident) => {
        $m! {
    /// Opens the dev portal window, creating it if it is not already open.
    dev_open_portal {
        group: "Dev · Diagnostics",
        danger: false,
        args: [],
    }
    /// Build, platform, active recorder backend, and every resolved path.
    dev_env_info {
        group: "Dev · Diagnostics",
        danger: false,
        args: [],
    }
    /// The Rust side's own list of production commands, for the drift check.
    dev_registered_commands {
        group: "Dev · Diagnostics",
        danger: false,
        args: [],
    }
    /// Reveals one of the app's directories in the OS file manager.
    dev_open_data_dir {
        group: "Dev · Diagnostics",
        danger: false,
        args: [
            { name: "which", kind: "string", default: "recordings", help: "recordings | app_data | fixtures | repo_fixtures", optional: false },
        ],
    }
    /// Opens a recording's file, or shows it in the OS file manager. Takes an id, not a path: the path comes off the row, so nothing the frontend holds decides which file is opened. Refuses a file that is no longer there.
    dev_reveal_recording {
        group: "Dev · Diagnostics",
        danger: false,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
            { name: "which", kind: "string", default: "folder", help: "play | folder", optional: false },
        ],
    }
    /// Opens a fixture in whatever the OS opens `.json` with, or shows it in the file manager. Confined to the two fixture roots, like `dev_fixture_read`, the panel's textarea is the wrong tool for a 99 KB capture.
    dev_open_fixture {
        group: "Dev · Fixtures",
        danger: false,
        args: [
            { name: "path", kind: "string", default: "", help: "", optional: false },
            { name: "which", kind: "string", default: "play", help: "play | folder", optional: false },
        ],
    }
        }
    };
}

/// The commands that run **wherever the library does**, which is the daemon.
///
/// These reach the database, the supervisor, the recorder or the log, so they
/// have to run in the process that owns those. Since WS3.7 they are reached by
/// name through `dev::dispatch` rather than registered individually, exactly
/// as the production commands are reached through `core::dispatch` — one `rpc`
/// frame, one table, no `generate_handler!` list to keep in step.
///
/// ## The `call:` field
///
/// The Rust signature, in the same shape `core`'s `dispatch_table!` uses: a
/// kind saying whether the function takes a `Ctx`, whether it is `async`, and
/// whether it returns a `Result`, then the arguments and the success type.
/// `dev::dispatch` expands it into the `match` arm that parses the JSON and
/// calls the function, so a wrong name or type here is a compile error at the
/// call rather than a manifest that quietly lies.
///
/// Every type is written as an absolute `$crate::` path, for the reason the
/// production table gives: a generator reading this has no module context to
/// resolve a bare name against. `$crate` rather than `crate` because this macro
/// is exported, so its tokens are expanded where the caller is, and `crate`
/// there would mean the caller's crate.
///
/// The `kind` in each `args:` entry is what the portal renders an input for,
/// and it is still spelled rather than derived from `call:`. The two say
/// different things: one is the Rust type, the other is the widget.
#[macro_export]
macro_rules! dev_rpc_command_table {
    ($m:ident) => {
        $m! {
    /// Everything the Overview panel polls, in one round trip.
    dev_health {
        group: "Dev · Diagnostics",
        danger: false,
        call: ctx_result() -> $crate::dev::info::DevHealth,
        args: [],
    }
    /// The backend log files that exist, newest first, including the rotated ones. Reports the missing ones too, "no log file" and "empty log file" are different answers.
    dev_log_files {
        group: "Dev · Tools",
        danger: false,
        call: bare_result() -> Vec<$crate::dev::log_api::LogFileInfo>,
        args: [],
    }
    /// One filtered window of a log file, newest matching lines first. Filtering happens in Rust: the file is capped at 5 MiB, which is far too much to hand a webview whole.
    dev_read_log {
        group: "Dev · Tools",
        danger: false,
        call: bare_result(query: $crate::dev::log_api::LogQuery) -> $crate::dev::log_api::LogPage,
        args: [
            { name: "query", kind: "json", default: "{'file': None, 'levels': [], 'hideTags': ['live-poll'], 'search': '', 'limit': 200}", help: "levels includes; hideTags excludes. Empty levels means every level, not none.", optional: false },
        ],
    }
    /// Live PRAGMA table_info for every browsable table, plus row counts.
    dev_schema {
        group: "Dev · Database",
        danger: false,
        call: ctx_result() -> Vec<$crate::dev::sql::TableSchema>,
        args: [],
    }
    /// A page of one table. Column names in order_by are validated against the schema.
    dev_table_page {
        group: "Dev · Database",
        danger: false,
        call: ctx_result(table: String, limit: Option<i64>, offset: Option<i64>, order_by: Option<String>) -> $crate::dev::sql::QueryResult,
        args: [
            { name: "table", kind: "string", default: "recordings", help: "", optional: false },
            { name: "limit", kind: "number", default: "100", help: "", optional: true },
            { name: "offset", kind: "number", default: "0", help: "", optional: true },
            { name: "orderBy", kind: "string", default: "", help: "e.g. \"started_at DESC\"", optional: true },
        ],
    }
    /// Arbitrary SQL against the live library database.
    dev_sql_query {
        group: "Dev · Database",
        danger: true,
        call: ctx_result(sql: String) -> $crate::dev::sql::QueryResult,
        args: [
            { name: "sql", kind: "string", default: "SELECT * FROM recordings LIMIT 20", help: "", optional: false },
        ],
    }
    /// Inserts one row, bypassing the typed API and its path upsert rule.
    dev_insert_row {
        group: "Dev · Database",
        danger: true,
        call: ctx_result(table: String, values: $crate::dev::sql::RowValues) -> i64,
        args: [
            { name: "table", kind: "string", default: "recordings", help: "", optional: false },
            { name: "values", kind: "json", default: "{}", help: "", optional: false },
        ],
    }
    /// Updates one row by id.
    dev_update_row {
        group: "Dev · Database",
        danger: true,
        call: ctx_result(table: String, id: serde_json::Value, values: $crate::dev::sql::RowValues) -> usize,
        args: [
            { name: "table", kind: "string", default: "recordings", help: "", optional: false },
            { name: "id", kind: "number", default: "", help: "", optional: false },
            { name: "values", kind: "json", default: "{}", help: "", optional: false },
        ],
    }
    /// Deletes one row by id. Without deleteFile, the next rescan re-imports the recording from its file.
    dev_delete_row {
        group: "Dev · Database",
        danger: true,
        call: ctx_result(table: String, id: serde_json::Value, delete_file: Option<bool>) -> usize,
        args: [
            { name: "table", kind: "string", default: "recordings", help: "", optional: false },
            { name: "id", kind: "number", default: "", help: "", optional: false },
            { name: "deleteFile", kind: "boolean", default: "False", help: "", optional: true },
        ],
    }
    /// Empties every table and restores the default retention policy.
    dev_reset_db {
        group: "Dev · Database",
        danger: true,
        call: ctx_result(also_clear_files: bool) -> $crate::dev::sql::ResetReport,
        args: [
            { name: "alsoClearFiles", kind: "boolean", default: "False", help: "", optional: false },
        ],
    }
    /// Generates recordings, markers, samples, and their files on disk.
    dev_seed_library {
        group: "Dev · Seed",
        danger: true,
        call: ctx_result(spec: $crate::dev::seed::SeedSpec) -> $crate::dev::seed::SeedReport,
        args: [
            { name: "spec", kind: "json", default: "{}", help: "", optional: false },
        ],
    }
    /// Removes every seeded recording and file. Captured recordings are untouched.
    dev_clear_seeded {
        group: "Dev · Seed",
        danger: true,
        call: ctx_result() -> $crate::dev::seed::ClearReport,
        args: [],
    }
    /// Dry run: exactly what enforcement would delete, and how many bytes it would free. Touches nothing.
    dev_retention_preview {
        group: "Dev · Retention",
        danger: false,
        call: ctx_result(policy: Option<$crate::db::RetentionPolicy>, now_millis: Option<i64>) -> $crate::dev::retention_api::RetentionPreview,
        args: [
            { name: "policy", kind: "json", default: "", help: "omit to use the saved policy", optional: true },
            { name: "nowMillis", kind: "number", default: "", help: "override the clock to test age rules", optional: true },
        ],
    }
    /// Feeds one event through the live supervisor. Really starts and stops the recorder.
    dev_dispatch_state_event {
        group: "Dev · Simulate",
        danger: true,
        call: ctx_result(event: $crate::dev::simulate::DevStateEvent) -> $crate::dev::simulate::DispatchReport,
        args: [
            { name: "event", kind: "json", default: "{'kind': 'gameflow_phase', 'phase': 'InProgress'}", help: "", optional: false },
        ],
    }
    /// Pushes one Live Client Data payload through the real marker/sample pipeline.
    dev_inject_snapshot {
        group: "Dev · Simulate",
        danger: false,
        call: ctx_result(snapshot: serde_json::Value) -> $crate::dev::simulate::InjectReport,
        args: [
            { name: "snapshot", kind: "json", default: "{}", help: "", optional: false },
        ],
    }
    /// The in-flight recording session, markers and samples accumulating right now.
    dev_session_snapshot {
        group: "Dev · Simulate",
        danger: false,
        call: ctx_plain() -> Option<$crate::state_machine::supervisor::DevSessionView>,
        args: [],
    }
    /// Plays a scripted game at a speed multiplier.
    dev_replay_start {
        group: "Dev · Simulate",
        danger: true,
        call: ctx_result(spec: $crate::dev::simulate::ReplaySpec) -> (),
        args: [
            { name: "spec", kind: "json", default: "{}", help: "", optional: false },
        ],
    }
    /// Aborts a running replay.
    dev_replay_stop {
        group: "Dev · Simulate",
        danger: false,
        call: bare_result() -> (),
        args: [],
    }
    /// Progress of the running replay.
    dev_replay_status {
        group: "Dev · Simulate",
        danger: false,
        call: bare_result() -> $crate::dev::simulate::ReplayStatus,
        args: [],
    }
    /// Raw GET against any LCU path. Needs the League Client running.
    dev_lcu_get {
        group: "Dev · Simulate",
        danger: false,
        call: bare_async_result(path: String) -> serde_json::Value,
        args: [
            { name: "path", kind: "string", default: "/lol-gameflow/v1/gameflow-phase", help: "", optional: false },
        ],
    }
    /// Resolves a champion id through the real asset-store lookup. 62 must come back as Wukong, MonkeyKing means the parse is reading `alias`. Needs the League Client running.
    dev_champion_name {
        group: "Dev · Simulate",
        danger: false,
        call: bare_async_result(champion_id: i64) -> Option<String>,
        args: [
            { name: "championId", kind: "number", default: "62", help: "62 is the one worth asking: its display name and its alias differ.", optional: false },
        ],
    }
    /// One shot at the post-game summary, end-of-game block, then match history, with no retries. Needs the League Client running.
    dev_fetch_match_summary {
        group: "Dev · Simulate",
        danger: false,
        call: bare_async_result(game_id: i64) -> $crate::lcu::MatchSummary,
        args: [
            { name: "gameId", kind: "number", default: "", help: "", optional: false },
        ],
    }
    /// Runs the whole deferred patch against a real client and writes the result to an existing recording. May block for up to a minute, that is the real retry schedule.
    dev_patch_match_summary {
        group: "Dev · Simulate",
        danger: true,
        call: ctx_async_result(recording_id: i64, game_id: i64, is_custom: bool, queue_id: Option<i64>) -> bool,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
            { name: "gameId", kind: "number", default: "", help: "", optional: false },
            { name: "isCustom", kind: "boolean", default: "False", help: "", optional: false },
            { name: "queueId", kind: "number", default: "420", help: "420 solo, 440 flex, gates the rank read", optional: false },
        ],
    }
    /// Raw allgamedata fetch. Only reachable while a game is running.
    dev_live_client_probe {
        group: "Dev · Simulate",
        danger: false,
        call: bare_async_result() -> serde_json::Value,
        args: [],
    }
    /// Capture flag, both fixture roots, and every fixture found under them.
    dev_fixtures_state {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_result() -> $crate::dev::fixtures_api::FixturesState,
        args: [],
    }
    /// Reads every captured payload back and reports what the parser did not understand, event names with no `classify_event` arm, events that failed to deserialize at all (with the JSON that broke them), fields that arrived as the wrong JSON type, including the ones a lenient reader silently absorbs, keys on events that nothing reads, and files that are not JSON at all. A report, not a validator: nothing here changes what the parser accepts. `HordeKill` sat in captures for months while Voidgrubs never became markers.
    dev_shape_report {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_result() -> $crate::dev::fixtures_api::ShapeReport,
        args: [],
    }
    /// One recording's whole story: the row, a named likely writer for every field that has more than one, marker and sample counts, the alignment offset, and the scoreboard and diagnostics parsed. Read-only, the actions that operate on a recording are their own commands.
    dev_recording_report {
        group: "Dev · Diagnostics",
        danger: false,
        call: ctx_result(recording_id: i64) -> $crate::dev::recording_api::RecordingReport,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
        ],
    }
    /// Asks the client about a recording's game and lays its answer beside the row's, field by field. The same one-shot `fetch_match_summary` the deferred patch uses, a disagreement here almost always means the wrong game id was matched. Read-only; `dev_patch_match_summary` is the one that acts on the answer.
    dev_recording_vs_lcu {
        group: "Dev · Diagnostics",
        danger: false,
        call: ctx_async_result(recording_id: i64) -> $crate::dev::recording_api::LcuComparison,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
        ],
    }
    /// Runs the backfill against one recording, the same candidate query, matching and refusals the whole-library pass uses, pointed at a single row. A row it has nothing to fill comes back with `scanned: 0` rather than an error.
    dev_backfill_recording {
        group: "Dev · Diagnostics",
        danger: true,
        call: ctx_async_result(recording_id: i64) -> $crate::backfill::BackfillReport,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
        ],
    }
    /// A player's ranked standing right now. Omit `puuid` to ask about yourself. **Take the puuid from a match document, never from an alias lookup**, that returns a name-derived v5 UUID the ranked ladder has nothing keyed by, so the lookup succeeds and this answers nothing, which looks exactly like an unranked player.
    dev_ranked_stats {
        group: "Dev · Diagnostics",
        danger: false,
        call: bare_async_result(puuid: Option<String>) -> serde_json::Value,
        args: [
            { name: "puuid", kind: "string", default: "", help: "blank = yourself", optional: false },
        ],
    }
    /// A whole lobby's rank from its puuids, the median standing, with a count of how many were known. Take the ten puuids from a captured `eog-stats-block` (`teams[].players[].puuid`). Players whose rank cannot be read are excluded rather than counted low.
    dev_lobby_rank {
        group: "Dev · Diagnostics",
        danger: false,
        call: bare_async_result(puuids: Vec<String>, queue: Option<String>) -> serde_json::Value,
        args: [
            { name: "puuids", kind: "json", default: "", help: "[\"puuid\", …]", optional: false },
            { name: "queue", kind: "string", default: "RANKED_SOLO_5x5", help: "", optional: false },
        ],
    }
    /// Measures an LP delta from two standings by hand, the bench for the one check that cannot be a unit test: whether the delta agrees with what the client's post-game screen showed. Reports the ladder positions as well as the answer, because a wrong delta is almost always a wrong position. Takes the client's own spelling.
    dev_lp_delta {
        group: "Dev · Diagnostics",
        danger: false,
        call: bare_plain(before: $crate::lcu::ranked::RankedEntry, after: $crate::lcu::ranked::RankedEntry, queue: Option<String>) -> serde_json::Value,
        args: [
            { name: "before", kind: "json", default: "{'tier': 'GOLD', 'division': 'IV', 'leaguePoints': 98}", help: "", optional: false },
            { name: "after", kind: "json", default: "{'tier': 'GOLD', 'division': 'III', 'leaguePoints': 8}", help: "", optional: false },
            { name: "queue", kind: "string", default: "RANKED_SOLO_5x5", help: "", optional: false },
        ],
    }
    /// Records **every** LCU WebSocket event to a JSONL file, unfiltered. A URI filter presupposes knowing which endpoint carries what you are hunting; this exists for the case where nothing does. Stops itself at 64 MiB rather than rotating, for a probe the start of a session is usually the part being looked for.
    dev_event_capture_start {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_async_result() -> $crate::dev::events::CaptureStatus,
        args: [],
    }
    /// Stops the running capture and reports what it wrote.
    dev_event_capture_stop {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_plain() -> $crate::dev::events::CaptureStatus,
        args: [],
    }
    /// Whether a capture is running, where it is writing, and how much it has written.
    dev_event_capture_status {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_plain() -> $crate::dev::events::CaptureStatus,
        args: [],
    }
    /// Which endpoints appeared in a capture, most frequent first. The half that makes a raw capture usable, a post-game window is thousands of frames across dozens of endpoints, and a list of URIs answers "which of these could carry it" in seconds where a 40 MB file does not.
    dev_event_uris {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_result(path: String) -> Vec<$crate::dev::events::UriCount>,
        args: [
            { name: "path", kind: "string", default: "", help: "a capture file from dev_event_capture_start", optional: false },
        ],
    }
    /// Reads one fixture. Confined to the two known fixture roots.
    dev_fixture_read {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_result(path: String) -> String,
        args: [
            { name: "path", kind: "string", default: "", help: "", optional: false },
        ],
    }
    /// Saves a payload as a fixture under the capture directory.
    dev_fixture_write {
        group: "Dev · Fixtures",
        danger: true,
        call: bare_result(group: String, name: String, contents: String) -> String,
        args: [
            { name: "group", kind: "string", default: "live-client", help: "", optional: false },
            { name: "name", kind: "string", default: "", help: "", optional: false },
            { name: "contents", kind: "string", default: "", help: "", optional: false },
        ],
    }
    /// Turns response capture on or off for the running process.
    dev_set_fixture_recording {
        group: "Dev · Fixtures",
        danger: false,
        call: bare_plain(enabled: bool) -> bool,
        args: [
            { name: "enabled", kind: "boolean", default: "True", help: "", optional: false },
        ],
    }
    /// Cuts the loading screen off a recording's file and rebases its markers and samples onto what is left. Finalize already does this; the command is for recordings made before it did, or one it skipped. A no-op on anything already trimmed.
    dev_trim_lead_in {
        group: "Dev · Tools",
        danger: true,
        call: ctx_result(recording_id: i64) -> $crate::trim::TrimReport,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "The row to cut. It needs samples: the loading screen's length is measured from them.", optional: false },
        ],
    }
        }
    };
}

/// Every `dev_*` command name: the ones the UI registers, then the ones the
/// daemon dispatches.
///
/// A `Vec` rather than a `&'static [..]` since WS3.7, because there are two
/// tables now and two `&'static` slices cannot be concatenated into a third
/// without a const-eval dance that would buy nothing: every element is still a
/// `&'static str`, and the only callers are the portal's catalogue and these
/// tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn dev_command_names() -> Vec<&'static str> {
    macro_rules! names {
        ($( $(#[doc = $doc:literal])* $name:ident { $($body:tt)* } )*) => {
            vec![ $( stringify!($name), )* ]
        };
    }
    let mut names: Vec<&'static str> = dev_ui_command_table!(names);
    names.extend(dev_rpc_command_table!(names));
    names
}

/// The names of the commands that run in the daemon, which is the half
/// `dev::dispatch` answers for.
///
/// Separate from `dev_command_names` because they answer different questions:
/// that one is "what does the portal know about", this one is "what can be
/// invoked over the pipe". The portal's catalogue needs the first; the
/// dispatcher's own test needs the second.
#[cfg_attr(not(test), allow(dead_code))]
pub fn dev_rpc_command_names() -> Vec<&'static str> {
    macro_rules! names {
        ($( $(#[doc = $doc:literal])* $name:ident { $($body:tt)* } )*) => {
            vec![ $( stringify!($name), )* ]
        };
    }
    dev_rpc_command_table!(names)
}

/// The dev surface as data, for the portal's command panel.
///
/// Built from the same tokens the handler is, so the panel cannot describe a
/// command that is not registered, and a command cannot be registered without
/// the panel knowing what it is.
#[cfg_attr(not(test), allow(dead_code))]
pub fn dev_command_manifest() -> Vec<DevCommandSpec> {
    // Two row shapes, because a row that names a `call:` is a row the daemon
    // dispatches and a row without one is the UI's. The *form* metadata is
    // identical in both, which is why the two arms build the same struct: what
    // the portal renders does not depend on which process answers.
    macro_rules! ui_specs {
        ($(
            $(#[doc = $doc:literal])*
            $name:ident {
                group: $group:literal,
                danger: $danger:literal,
                args: [ $( {
                    name: $an:literal,
                    kind: $ak:literal,
                    default: $ad:literal,
                    help: $ah:literal,
                    optional: $ao:literal
                } ),* $(,)? ],
            }
        )*) => {
            vec![ $( DevCommandSpec {
                name: stringify!($name),
                group: $group,
                danger: $danger,
                description: concat!($($doc),*),
                over_rpc: false,
                args: &[ $( DevArgSpec {
                    name: $an, kind: $ak, default: $ad, help: $ah, optional: $ao,
                }, )* ],
            }, )* ]
        };
    }
    macro_rules! rpc_specs {
        ($(
            $(#[doc = $doc:literal])*
            $name:ident {
                group: $group:literal,
                danger: $danger:literal,
                call: $kind:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty,
                args: [ $( {
                    name: $an:literal,
                    kind: $ak:literal,
                    default: $ad:literal,
                    help: $ah:literal,
                    optional: $ao:literal
                } ),* $(,)? ],
            }
        )*) => {
            vec![ $( DevCommandSpec {
                name: stringify!($name),
                group: $group,
                danger: $danger,
                description: concat!($($doc),*),
                over_rpc: true,
                args: &[ $( DevArgSpec {
                    name: $an, kind: $ak, default: $ad, help: $ah, optional: $ao,
                }, )* ],
            }, )* ]
        };
    }
    let mut specs: Vec<DevCommandSpec> = dev_ui_command_table!(ui_specs);
    specs.extend(dev_rpc_command_table!(rpc_specs));
    specs
}

/// How the portal renders a form for a *production* command.
///
/// The description for these lives on the `dispatch_table!` row as a doc
/// comment and reaches the portal through `core::dispatch::command_descriptions`,
/// so it is not repeated here. What is here is the part the dispatch table has
/// no room for: which panel heading a command sorts under, whether it is
/// destructive, and what to pre-fill each input with.
///
/// **This is a second list, and that is a deliberate trade.** The honest
/// alternative was extending `dispatch_table!`'s matcher, which drives live
/// command dispatch, to carry form metadata it has no other use for. Keeping
/// the portal's concerns in the portal's module and pinning the two together
/// with `every_production_command_has_form_metadata` costs one test and fails
/// `cargo test` the moment they disagree, rather than lighting a banner in a
/// dev-only UI that nobody is looking at.
#[macro_export]
macro_rules! production_form_table {
    ($m:ident) => {
        $m! {
    backfill_match_metadata {
        group: "Library",
        danger: false,
        args: [],
    }
    check_for_update {
        group: "Updates",
        danger: false,
        args: [],
    }
    delete_recording {
        group: "Library",
        danger: true,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
        ],
    }
    extract_audio_track {
        group: "Audio",
        danger: false,
        args: [
            { name: "recordingPath", kind: "string", default: "", help: "", optional: false },
            { name: "trackIndex", kind: "number", default: "1", help: "", optional: false },
        ],
    }
    game_state_status {
        group: "League",
        danger: false,
        args: [],
    }
    get_audio_preset {
        group: "Audio",
        danger: false,
        args: [],
    }
    get_autostart {
        group: "Settings",
        danger: false,
        args: [],
    }
    get_disk_usage {
        group: "Disk",
        danger: false,
        args: [],
    }
    get_recording_markers {
        group: "Library",
        danger: false,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "recordings.id", optional: false },
        ],
    }
    get_recording_samples {
        group: "Library",
        danger: false,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "recordings.id", optional: false },
        ],
    }
    get_recordings_dir {
        group: "Disk",
        danger: false,
        args: [],
    }
    get_retention_policy {
        group: "Disk",
        danger: false,
        args: [],
    }
    get_ui_prefs {
        group: "Settings",
        danger: false,
        args: [],
    }
    get_update_status {
        group: "Updates",
        danger: false,
        args: [],
    }
    install_update {
        group: "Updates",
        danger: true,
        args: [],
    }
    is_recording {
        group: "Recorder",
        danger: false,
        args: [],
    }
    lcu_status {
        group: "League",
        danger: false,
        args: [],
    }
    list_audio_inputs {
        group: "Audio",
        danger: false,
        args: [],
    }
    list_recordings {
        group: "Library",
        danger: false,
        args: [],
    }
    open_recordings_folder {
        group: "Disk",
        danger: false,
        args: [],
    }
    preview_retention_policy {
        group: "Disk",
        danger: false,
        args: [
            { name: "policy", kind: "json", default: "{'max_total_bytes': 53687091200, 'max_age_days': 30}", help: "", optional: false },
        ],
    }
    rescan_recordings {
        group: "Library",
        danger: true,
        args: [],
    }
    resolve_icons {
        group: "Library",
        danger: false,
        args: [
            { name: "request", kind: "json", default: "{'champions': ['Wukong'], 'items': [3089], 'spells': ['Flash'], 'runes': [8112]}", help: "Four lists: champions, items, spells, runes. Any of them may be omitted.", optional: false },
        ],
    }
    set_audio_preset {
        group: "Audio",
        danger: true,
        args: [
            { name: "preset", kind: "json", default: "{'preset': 'game'}", help: "e.g. {\"preset\":\"game_mic_discord\"} or {\"preset\":\"game\"}", optional: false },
        ],
    }
    set_autostart {
        group: "Settings",
        danger: true,
        args: [
            { name: "enabled", kind: "boolean", default: "False", help: "", optional: false },
        ],
    }
    set_pinned {
        group: "Library",
        danger: true,
        args: [
            { name: "recordingId", kind: "number", default: "", help: "", optional: false },
            { name: "pinned", kind: "boolean", default: "True", help: "", optional: false },
        ],
    }
    set_retention_policy {
        group: "Disk",
        danger: true,
        args: [
            { name: "policy", kind: "json", default: "{'max_total_bytes': 53687091200, 'max_age_days': 30}", help: "", optional: false },
        ],
    }
    set_ui_pref {
        group: "Settings",
        danger: true,
        args: [
            { name: "key", kind: "string", default: "theme", help: "", optional: false },
            { name: "value", kind: "string", default: "dark", help: "", optional: false },
        ],
    }
    start_recording {
        group: "Recorder",
        danger: true,
        args: [],
    }
    stop_recording {
        group: "Recorder",
        danger: true,
        args: [],
    }
        }
    };
}

/// Form metadata for the production commands, keyed by name.
#[cfg_attr(not(test), allow(dead_code))]
pub fn production_form_manifest() -> &'static [DevCommandSpec] {
    macro_rules! specs {
        ($(
            $name:ident {
                group: $group:literal,
                danger: $danger:literal,
                args: [ $( {
                    name: $an:literal,
                    kind: $ak:literal,
                    default: $ad:literal,
                    help: $ah:literal,
                    optional: $ao:literal
                } ),* $(,)? ],
            }
        )*) => {
            &[ $( DevCommandSpec {
                name: stringify!($name),
                group: $group,
                danger: $danger,
                // Filled from the dispatch table's doc comments by the
                // generator; empty here on purpose so there is one home for it.
                description: "",
                // Every production command goes through `rpc`, and has since
                // WS2.7. This table is only their form metadata.
                over_rpc: true,
                args: &[ $( DevArgSpec {
                    name: $an, kind: $ak, default: $ad, help: $ah, optional: $ao,
                }, )* ],
            }, )* ]
        };
    }
    production_form_table!(specs)
}

#[cfg(test)]
mod tests {
    /// The property the shape exists for: the list that becomes
    /// `generate_handler!` and the list the portal is told about are the same
    /// tokens, so they cannot disagree.
    #[test]
    fn the_names_are_dev_commands_and_are_unique() {
        let names = super::dev_command_names();
        assert!(!names.is_empty());
        for n in &names {
            assert!(n.starts_with("dev_"), "{n} is in the dev list but is not a dev_* command");
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "a command is listed twice");
    }

    /// The manifest and the name list come from one table, so this is really a
    /// check that both callbacks read every row.
    #[test]
    fn the_manifest_covers_every_name() {
        let names = super::dev_command_names();
        let manifest = super::dev_command_manifest();
        assert_eq!(names.len(), manifest.len());
        for (n, spec) in names.iter().zip(&manifest) {
            assert_eq!(n, &spec.name);
        }
    }

    /// A command with no help text is a command nobody can use from the portal
    /// without reading the source, which is what this table exists to avoid.
    #[test]
    fn every_command_says_what_it_does() {
        for spec in super::dev_command_manifest() {
            assert!(!spec.description.is_empty(), "{} has no description", spec.name);
            assert!(!spec.group.is_empty(), "{} has no group", spec.name);
        }
    }
    /// The second list, pinned to the first. `dispatch_table!` is the authority
    /// on which production commands exist; this asserts the portal knows how to
    /// render every one of them and invents none.
    #[test]
    fn every_production_command_has_form_metadata() {
        use std::collections::BTreeSet;
        // `command_names()` is the dispatch table, which is not quite the whole
        // production surface: `open_recordings_folder` drives the desktop shell
        // and stays directly registered, so it has no table row. This is the
        // same definition `dev_registered_commands` uses.
        let mut declared: BTreeSet<&str> = crate::core::command_names().iter().copied().collect();
        declared.insert("open_recordings_folder");
        let described: BTreeSet<&str> =
            super::production_form_manifest().iter().map(|s| s.name).collect();
        let missing: Vec<_> = declared.difference(&described).collect();
        let invented: Vec<_> = described.difference(&declared).collect();
        assert!(missing.is_empty(), "no form metadata for {missing:?}");
        assert!(invented.is_empty(), "form metadata for commands that do not exist: {invented:?}");
    }

    /// Every production command's help text comes from its table row, so a row
    /// without one leaves the portal with a blank description.
    #[test]
    fn every_production_command_says_what_it_does() {
        for (name, description) in crate::core::dispatch::command_descriptions() {
            assert!(!description.is_empty(), "{name} has no doc comment on its table row");
        }
    }
}
