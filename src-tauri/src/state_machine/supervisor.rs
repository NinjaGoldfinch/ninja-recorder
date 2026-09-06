//! Async orchestration around the pure `StateMachine`: spawns/aborts the
//! lockfile, gameflow, and Live Client Data watchers per `Action`, and
//! drives the `Recorder` and the marker pipeline (`live_client`).
//! DEVELOPMENT.md §3.4.
//!
//! Unlike `machine.rs`, this glue can't be meaningfully unit-tested
//! without a real LCU/Live Client Data connection — no League client is
//! installed on the machine this was written on. It's kept as thin as
//! possible over the well-tested pure transition function specifically so
//! the untested surface is small: `execute` mostly just spawns/aborts
//! tasks and calls the already-tested `Recorder` trait methods.

use super::machine::{Action, GameState, StateEvent, StateMachine};
use crate::db::{self, Db};
use crate::lcu;
use crate::live_client::{self, AllGameData, Marker, MarkerTracker, TimeAlignment};
use crate::live_client::team_diff;
use crate::recorder::{RecordConfig, Recorder};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;

#[derive(Debug, Clone, Serialize)]
pub struct SessionMarker {
    #[serde(flatten)]
    pub marker: Marker,
    pub video_time_s: f64,
}

/// One 1 Hz sample of the team-advantage series, time-aligned to the video
/// the same way markers are. Kept in memory for the duration of the
/// recording and flushed to the DB in one transaction at finalize.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSample {
    pub game_time_s: f64,
    pub video_time_s: f64,
    /// `None` when the active player couldn't be matched in `allPlayers` —
    /// see `live_client::events::team_diff` for why that isn't guessed.
    pub diff: Option<live_client::TeamDiff>,
    pub our_gold: f64,
    pub our_level: i64,
}

/// The one seam between the supervisor and whatever is listening. Named so
/// the `Mutex<Option<Box<dyn ...>>>` stack stays readable, and so the process
/// split has one type to change.
type EventNotifier = Box<dyn Fn(SupervisorEvent) + Send + Sync>;

/// Something the supervisor wants the rest of the app to know about.
///
/// A single enum behind a single notifier, rather than one callback per
/// signal: when the recorder moves into its own process this seam becomes a
/// socket write, and there should be exactly one place to change.
#[derive(Debug, Clone)]
pub enum SupervisorEvent {
    /// The VOD library changed behind the frontend's back.
    LibraryChanged,
    /// Capture began.
    RecordingStarted,
    /// A recording was finalized. Carries what was written so a notification
    /// can describe it without going back to the database.
    Finalized(FinalizedRecording),
    /// A recording could not be started or could not be finished. Carries a
    /// message fit to show the user — they are in a game and cannot see the
    /// window.
    RecordingFailed(String),
}

/// What the supervisor learned about the most recently finished recording,
/// surfaced to the frontend via `game_state_status`. Also written to the
/// SQLite VOD library (`db`) — `recording_id` is `None` only if that write
/// itself failed, so the in-memory copy still isn't lost.
#[derive(Debug, Clone, Serialize)]
pub struct FinalizedRecording {
    pub recording_id: Option<i64>,
    pub path: String,
    pub markers: Vec<SessionMarker>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupervisorStatus {
    pub state: GameState,
    pub last_finalized: Option<FinalizedRecording>,
    /// Seconds since capture actually started, or `None` when nothing is
    /// recording. Read from the session rather than timed by the UI: the
    /// window can be opened part-way through a game, and a counter that
    /// starts at zero when the UI first looks would misreport how much has
    /// been captured.
    pub recording_elapsed_s: Option<f64>,
}

struct RecordingSession {
    tracker: MarkerTracker,
    markers: Vec<SessionMarker>,
    samples: Vec<SessionSample>,
    alignment: Option<TimeAlignment>,
    record_started_at: Instant,
    /// Wall-clock capture alongside `record_started_at` — `Instant` is
    /// monotonic only, not convertible to a real timestamp, but the DB's
    /// `recordings.started_at` column needs one.
    started_at_millis: i64,
}

pub struct Supervisor {
    machine: Mutex<StateMachine>,
    recorder: Arc<Mutex<Box<dyn Recorder>>>,
    recordings_dir: PathBuf,
    db: Arc<Db>,
    gameflow_task: Mutex<Option<JoinHandle<()>>>,
    live_client_task: Mutex<Option<JoinHandle<()>>>,
    session: Mutex<Option<RecordingSession>>,
    last_finalized: Mutex<Option<FinalizedRecording>>,
    /// Set once at startup via `set_event_notifier`, rather
    /// than taken in `new`, so the unit tests below can still build a
    /// `Supervisor` without a Tauri runtime. `None` simply means nothing
    /// is emitted.
    ///
    /// Deliberately a boxed closure rather than an `AppHandle`. Holding
    /// the handle here and calling `Emitter::emit` on it made Tauri's Wry
    /// window machinery *reachable* from this module — and this module has
    /// unit tests, so the linker could no longer discard it from the test
    /// harness. That dragged the whole Win32 GUI stack (`user32`, `gdi32`,
    /// `comctl32`, `ole32`, `shell32`, …) into the test executable's
    /// import table, and a `cargo test` binary carries no application
    /// manifest — so Windows resolved `comctl32.dll` to the v5
    /// side-by-side assembly, which lacks the v6 exports Tauri links
    /// against. The test binary then died at load with
    /// STATUS_ENTRYPOINT_NOT_FOUND before running a single test.
    /// Type-erasing the emit keeps all of that inside `lib.rs`'s `run()`,
    /// which stays dead code — and so gets stripped — in a test build.
    on_event: Mutex<Option<EventNotifier>>,
}

impl Supervisor {
    pub fn new(
        recorder: Arc<Mutex<Box<dyn Recorder>>>,
        recordings_dir: PathBuf,
        db: Arc<Db>,
    ) -> Arc<Self> {
        Arc::new(Self {
            machine: Mutex::new(StateMachine::new()),
            recorder,
            recordings_dir,
            db,
            gameflow_task: Mutex::new(None),
            live_client_task: Mutex::new(None),
            session: Mutex::new(None),
            last_finalized: Mutex::new(None),
            on_event: Mutex::new(None),
        })
    }

    /// Gives the supervisor a way to tell the frontend the library
    /// changed. Called once from `lib.rs`'s `setup`, after the app is
    /// built. See `on_event` for why this takes a closure and not an
    /// `AppHandle`.
    ///
    /// One notifier over an event enum rather than one per signal: the
    /// process split will replace what sits behind this seam with a socket
    /// write, and having a single seam to replace is the point.
    pub fn set_event_notifier(&self, notify: EventNotifier) {
        *self.on_event.lock().unwrap() = Some(notify);
    }

    fn emit(&self, event: SupervisorEvent) {
        if let Some(notify) = self.on_event.lock().unwrap().as_ref() {
            notify(event);
        }
    }

    /// Tells the frontend the VOD library changed on disk. Until this
    /// existed the app had no backend-to-frontend push at all, so a
    /// recording finalized by the supervisor stayed invisible until the
    /// user happened to press Refresh.
    fn emit_library_changed(&self) {
        self.emit(SupervisorEvent::LibraryChanged);
    }

    pub fn status(&self) -> SupervisorStatus {
        SupervisorStatus {
            state: self.machine.lock().unwrap().state.clone(),
            last_finalized: self.last_finalized.lock().unwrap().clone(),
            recording_elapsed_s: self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .map(|s| s.record_started_at.elapsed().as_secs_f64()),
        }
    }

    /// Starts the always-on lockfile watch. Everything else (gameflow
    /// watch, Live Client Data polling, recording) is started/stopped by
    /// state transitions from here on. Runs for the app's lifetime.
    /// Finalizes an in-flight recording so the process can exit without
    /// losing the game, returning whether there was one.
    ///
    /// The tray's Quit needs this: quitting mid-match would otherwise leave a
    /// fragmented MP4 on disk with no `recordings` row, recoverable only by
    /// the next startup's `reconcile` and stripped of its markers.
    ///
    /// **Blocking.** This is the same finalize the state machine runs — the
    /// recorder stop, the ffmpeg remux, the DB write and a retention sweep —
    /// so it must not be called from the main thread, which on the tray path
    /// would freeze the menu and the whole event loop for seconds.
    pub fn finalize_for_shutdown(&self) -> bool {
        if self.session.lock().unwrap().is_none() {
            return false;
        }
        self.stop_recording();
        true
    }

    pub fn start(self: &Arc<Self>) {
        let sup = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            lcu::lockfile::watch(Duration::from_secs(2), move |state| {
                sup.dispatch(StateEvent::LockfileChanged(state));
            })
            .await;
        });
    }

    /// Feeds one event through the state machine and executes whatever
    /// actions it returns, then always tries `FinalizeComplete` as a
    /// follow-up. That's only a real transition when we just entered
    /// `Finalizing` (whose finalize actions — stop recorder, stop
    /// pollers — run synchronously above, so completing immediately is
    /// correct); in every other state it hits the state machine's
    /// wildcard arm and is a no-op. This blanket approach means callers
    /// never need to remember to send `FinalizeComplete` themselves, and
    /// avoids re-entrant locking that a nested `dispatch` call from
    /// inside `execute` would cause.
    fn dispatch(self: &Arc<Self>, event: StateEvent) {
        self.dispatch_one(event);
        self.dispatch_one(StateEvent::FinalizeComplete);
    }

    fn dispatch_one(self: &Arc<Self>, event: StateEvent) {
        let (actions, state) = {
            let mut machine = self.machine.lock().unwrap();
            (machine.handle(event), machine.state.clone())
        };
        for action in actions {
            self.execute(action);
        }
        self.sync_capture_backend(&state);
    }

    /// Keeps the capture backend's resident cost tied to whether a game is
    /// plausible: warm from the moment the League client is running, cold
    /// once it isn't (DEVELOPMENT.md §1.2).
    ///
    /// Driven off the resulting *state* rather than off `Action`s, because
    /// the actions don't say it. A client restart emits a stop and a start
    /// of the gameflow watch while staying in `ClientRunning`, so acting on
    /// those would tear libobs down and bring it straight back up for
    /// nothing. Reading the state makes that a no-op, and makes this
    /// idempotent — which matters because `dispatch` runs us twice.
    fn sync_capture_backend(&self, state: &GameState) {
        // Blocks on the recorder mutex, which `stop_recording` holds for the
        // whole finalize. So a lockfile-watch dispatch landing mid-finalize
        // waits a few seconds here. Deliberate: `try_lock` would be
        // non-blocking but could silently skip the `release` at `Idle`, and
        // nothing would dispatch again until the client came back, leaving
        // the backend warm indefinitely — the exact thing this exists to
        // prevent. Delaying a background poll is the cheaper trade.
        let mut recorder = self.recorder.lock().unwrap();
        if matches!(state, GameState::Idle) {
            recorder.release();
        } else if let Err(e) = recorder.prepare() {
            // Not fatal: `start` retries the bring-up itself, and a warning
            // now is more useful than silence until someone tries to record.
            eprintln!("[state_machine] capture backend not ready: {e}");
        }
    }

    fn execute(self: &Arc<Self>, action: Action) {
        match action {
            Action::StartGameflowWatch(info) => self.start_gameflow_watch(info),
            Action::StopGameflowWatch => self.stop_gameflow_watch(),
            Action::StartLiveClientPoll => self.start_live_client_poll(),
            Action::StopLiveClientPoll => self.stop_live_client_poll(),
            Action::StartRecording => self.start_recording(),
            Action::StopRecording => self.stop_recording(),
        }
    }

    fn start_gameflow_watch(self: &Arc<Self>, lockfile: lcu::LockfileInfo) {
        let sup = Arc::clone(self);
        let handle = tauri::async_runtime::spawn(async move {
            let client = match lcu::LcuHttpClient::new(&lockfile) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[state_machine] failed to build LCU client: {e}");
                    return;
                }
            };
            lcu::gameflow::watch(&lockfile, &client, Duration::from_secs(1), {
                let sup = Arc::clone(&sup);
                move |update| sup.dispatch(StateEvent::GameflowPhase(update.phase))
            })
            .await;
        });
        *self.gameflow_task.lock().unwrap() = Some(handle);
    }

    fn stop_gameflow_watch(&self) {
        if let Some(handle) = self.gameflow_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    fn start_live_client_poll(self: &Arc<Self>) {
        let sup = Arc::clone(self);
        let handle = tauri::async_runtime::spawn(async move {
            let client = match live_client::LiveClientDataClient::new() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[state_machine] failed to build Live Client Data client: {e}");
                    return;
                }
            };
            live_client::poller::watch(
                &client,
                Duration::from_secs(1),
                {
                    let sup = Arc::clone(&sup);
                    move |snapshot| sup.on_snapshot(snapshot)
                },
                {
                    let sup = Arc::clone(&sup);
                    move || sup.dispatch(StateEvent::LiveClientDown)
                },
            )
            .await;
        });
        *self.live_client_task.lock().unwrap() = Some(handle);
    }

    fn stop_live_client_poll(&self) {
        if let Some(handle) = self.live_client_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Every successful poll: (1) tells the state machine Live Client Data
    /// is reachable — a no-op unless we're still `WaitingForGame`, in
    /// which case it starts recording; (2) if we're recording, extracts
    /// and time-aligns any markers new since the last poll.
    fn on_snapshot(self: &Arc<Self>, snapshot: AllGameData) {
        self.dispatch(StateEvent::LiveClientUp);

        let mut guard = self.session.lock().unwrap();
        let Some(session) = guard.as_mut() else {
            return;
        };

        if session.alignment.is_none() {
            let elapsed = session.record_started_at.elapsed().as_secs_f64();
            session.alignment = Some(TimeAlignment::new(snapshot.game_data.game_time, elapsed));
        }
        let alignment = session.alignment.expect("just set above if it was None");

        let fresh = session.tracker.ingest(&snapshot);
        session
            .markers
            .extend(fresh.into_iter().map(|marker| SessionMarker {
                video_time_s: alignment.video_time_s(marker.game_time_s),
                marker,
            }));

        // Advantage-curve sample. Skipped unless game time actually moved:
        // the poller re-fetches the same payload during loading screens and
        // pauses, and a flat run of identical timestamps would draw a
        // vertical artefact through the graph.
        let game_time_s = snapshot.game_data.game_time;
        let moved = session
            .samples
            .last()
            .is_none_or(|last| game_time_s > last.game_time_s);
        if moved {
            let active = snapshot.active_player.as_ref();
            session.samples.push(SessionSample {
                game_time_s,
                video_time_s: alignment.video_time_s(game_time_s),
                diff: team_diff(&snapshot),
                our_gold: active.map(|p| p.current_gold).unwrap_or(0.0),
                our_level: active.map(|p| p.level).unwrap_or(0),
            });
        }
    }

    /// Executes `Action::StartRecording`. The state machine has already
    /// optimistically transitioned to `Recording` by the time this runs —
    /// if `Recorder::start` fails here (only reachable today via the dev
    /// panel's manual start button racing the automatic path, since
    /// nothing else calls it), the state machine's belief and the
    /// recorder's actual state diverge. Known gap, logged loudly rather
    /// than silently wrong; not expected to occur outside that manual
    /// double-start collision.
    fn start_recording(&self) {
        if !crate::retention::has_room_to_record(&self.recordings_dir) {
            eprintln!("[state_machine] refusing to start recording: insufficient free disk space");
            self.emit(SupervisorEvent::RecordingFailed(
                "not enough free disk space to record this game".into(),
            ));
            return;
        }

        let started_at_millis = timestamp_millis();
        // Read the preset per recording rather than caching it at startup:
        // the user can change what gets captured between games, and the
        // next game should honour that without a restart.
        let config = RecordConfig {
            output_dir: self.recordings_dir.clone(),
            file_stem: format!("recording-{started_at_millis}"),
            audio: self.db.get_audio_preset().unwrap_or_else(|e| {
                eprintln!("[state_machine] could not read the audio preset ({e}), using the default");
                Default::default()
            }),
        };
        match self.recorder.lock().unwrap().start(config) {
            Ok(()) => {
                *self.session.lock().unwrap() = Some(RecordingSession {
                    tracker: MarkerTracker::new(),
                    markers: Vec::new(),
                    samples: Vec::new(),
                    alignment: None,
                    record_started_at: Instant::now(),
                    started_at_millis,
                });
                self.emit(SupervisorEvent::RecordingStarted);
            }
            Err(e) => {
                eprintln!("[state_machine] failed to start recording: {e}");
                // Silence here means the user finds out after the game, when
                // the VOD isn't in the library.
                self.emit(SupervisorEvent::RecordingFailed(format!(
                    "the recording could not be started: {e}"
                )));
            }
        }
    }

    /// Executes `Action::StopRecording`: stops the recorder, then writes
    /// the recording + its markers to the VOD library DB (DEVELOPMENT.md
    /// §4). A DB write failure is logged but doesn't lose the in-memory
    /// copy — `last_finalized` is still set either way, just with
    /// `recording_id: None`, so nothing already captured is thrown away
    /// even if the row never made it to disk.
    fn stop_recording(&self) {
        let session = self.session.lock().unwrap().take();
        match self.recorder.lock().unwrap().stop() {
            Ok(output) => {
                let path = output.path;
                let markers = session.as_ref().map(|s| s.markers.clone()).unwrap_or_default();
                let samples = session.as_ref().map(|s| s.samples.clone()).unwrap_or_default();
                let started_at = session
                    .as_ref()
                    .map(|s| s.started_at_millis)
                    .unwrap_or_else(timestamp_millis);
                let path_str = path.display().to_string();
                let size_bytes = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);

                // Serialized from what the backend reports it wrote, not
                // from the preset we asked for — see `RecordingOutput`.
                let audio_tracks_json = match serde_json::to_string(&output.audio) {
                    Ok(json) => Some(json),
                    Err(e) => {
                        eprintln!("[state_machine] could not encode the audio track layout: {e}");
                        None
                    }
                };

                let recording_id = match self.db.insert_recording(&db::NewRecording {
                    path: path_str.clone(),
                    started_at,
                    size_bytes,
                    audio_tracks_json,
                    ..Default::default()
                }) {
                    Ok(id) => {
                        let new_markers: Vec<db::NewMarker> = markers
                            .iter()
                            .map(|m| db::NewMarker {
                                game_time_s: m.marker.game_time_s,
                                video_time_s: m.video_time_s,
                                kind: m.marker.kind.as_str().to_string(),
                                payload_json: m.marker.payload.to_string(),
                            })
                            .collect();
                        if let Err(e) = self.db.insert_markers(id, &new_markers) {
                            eprintln!("[state_machine] failed to insert markers for recording {id}: {e}");
                        }

                        let new_samples: Vec<db::NewSample> = samples
                            .iter()
                            .map(|s| db::NewSample {
                                game_time_s: s.game_time_s,
                                video_time_s: s.video_time_s,
                                our_team: s.diff.as_ref().map(|d| d.our_team.clone()),
                                gold_diff_est: s.diff.as_ref().map(|d| d.gold_diff_est),
                                kill_diff: s.diff.as_ref().map(|d| d.kill_diff),
                                cs_diff: s.diff.as_ref().map(|d| d.cs_diff),
                                our_gold: Some(s.our_gold),
                                our_level: Some(s.our_level),
                            })
                            .collect();
                        if let Err(e) = self.db.insert_samples(id, &new_samples) {
                            eprintln!("[state_machine] failed to insert samples for recording {id}: {e}");
                        }
                        Some(id)
                    }
                    Err(e) => {
                        eprintln!("[state_machine] failed to insert recording row: {e}");
                        None
                    }
                };

                *self.last_finalized.lock().unwrap() = Some(FinalizedRecording {
                    recording_id,
                    path: path_str,
                    markers,
                });

                // Retention (DEVELOPMENT.md §6): enforced right
                // after every finalize, in addition to app-start
                // (lib.rs's `setup`) — this is what actually keeps disk
                // usage bounded during a long play session where the app
                // never restarts.
                match self.db.get_retention_policy() {
                    Ok(policy) => match crate::retention::enforce_now(&self.db, &policy) {
                        Ok(report) if !report.deleted.is_empty() => println!(
                            "[retention] post-finalize enforcement: removed {} recording(s), freed {} bytes",
                            report.deleted.len(),
                            report.freed_bytes
                        ),
                        Ok(_) => {}
                        Err(e) => eprintln!("[retention] post-finalize enforcement failed: {e}"),
                    },
                    Err(e) => eprintln!("[retention] failed to load policy: {e}"),
                }

                // After the row, its markers/samples, and any retention
                // deletions — one notification for the whole finalize.
                self.emit_library_changed();
                // Separate from the library signal because it carries what was
                // written: the frontend refreshes off the first, a tray
                // notification describes the second.
                //
                // Note this still runs under the recorder lock, like the DB
                // writes above — recorder-then-db, so no lock-order inversion.
                // Whatever is behind this notifier must stay quick; it is not
                // the place to do work.
                if let Some(finalized) = self.last_finalized.lock().unwrap().clone() {
                    self.emit(SupervisorEvent::Finalized(finalized));
                }
            }
            Err(e) => {
                eprintln!("[state_machine] failed to stop recording: {e}");
                // The user is mid-game with the window closed; a failed
                // finalize is the one thing they cannot otherwise discover.
                self.emit(SupervisorEvent::RecordingFailed(format!(
                    "the recording could not be finished: {e}"
                )));
            }
        }
    }
}

/// Dev-portal entry points into the otherwise-private async glue. These
/// exist because `machine.rs`'s pure transition function is well covered
/// by unit tests while *this* file — the part that actually spawns
/// watchers and drives the recorder — has never run against a real LCU
/// (DEVELOPMENT.md §3.4). Feeding it synthetic events from the dev portal
/// is the first exercise it gets.
#[cfg(feature = "devtools")]
impl Supervisor {
    /// Feeds one event through the real `dispatch`, executing whatever
    /// actions it returns for real: watchers are spawned, the recorder is
    /// started and stopped, DB rows are written. That is the point — this
    /// is not a dry run.
    pub fn dev_dispatch(self: &Arc<Self>, event: StateEvent) {
        self.dispatch(event);
    }

    /// Feeds one Live Client Data payload through the real marker/sample
    /// pipeline, exactly as the poller would.
    pub fn dev_on_snapshot(self: &Arc<Self>, snapshot: AllGameData) {
        self.on_snapshot(snapshot);
    }

    /// A read-only view of the in-flight recording session. Nothing else
    /// exposes this — `SupervisorStatus` only carries the *last finalized*
    /// recording, so markers and samples accumulating during a recording
    /// are otherwise invisible until it ends.
    pub fn dev_session_view(&self) -> Option<DevSessionView> {
        let guard = self.session.lock().unwrap();
        let session = guard.as_ref()?;
        Some(DevSessionView {
            marker_count: session.markers.len(),
            sample_count: session.samples.len(),
            alignment_offset_s: session.alignment.map(|a| a.offset_s()),
            elapsed_s: session.record_started_at.elapsed().as_secs_f64(),
            started_at_millis: session.started_at_millis,
            recent_markers: session.markers.iter().rev().take(20).cloned().collect(),
            last_sample: session.samples.last().cloned(),
        })
    }

    /// Emits `library-changed` on behalf of the dev commands, which mutate
    /// the DB directly rather than going through a finalize.
    pub fn dev_emit_library_changed(&self) {
        self.emit_library_changed();
    }
}

/// See `Supervisor::dev_session_view`. `alignment_offset_s` is `None`
/// until the first snapshot arrives — recording starts before Live Client
/// Data is reachable, so there is always a window where the session
/// exists but has no game-time-to-video-time mapping yet.
#[cfg(feature = "devtools")]
#[derive(Debug, Clone, Serialize)]
pub struct DevSessionView {
    pub marker_count: usize,
    pub sample_count: usize,
    pub alignment_offset_s: Option<f64>,
    pub elapsed_s: f64,
    pub started_at_millis: i64,
    /// Newest first, capped — this is polled at 1 Hz by the portal.
    pub recent_markers: Vec<SessionMarker>,
    pub last_sample: Option<SessionSample>,
}

fn timestamp_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    //! `start_recording`/`stop_recording` are the one piece of this
    //! module's async glue that genuinely can be tested without a live
    //! LCU/Live Client Data connection: they only touch the (already
    //! StubRecorder-backed) `Recorder` trait and the DB, both already
    //! exercised elsewhere. Everything else here (gameflow/lockfile/
    //! live-client watchers) is deliberately left untested per this
    //! file's header — no League client is installed on this machine.
    use super::*;
    use crate::db::Db;
    use crate::live_client::MarkerKind;
    use crate::recorder::stub::StubRecorder;
    use crate::recorder::RecorderError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts `prepare`/`release` so the capture-backend lifecycle can be
    /// asserted on. The real bring-up only exists on Windows, so this is
    /// the only place the *policy* — warm with the client, cold at idle —
    /// is testable at all. Counters are shared with the test rather than
    /// read back out of the `Box<dyn Recorder>`, which can't be downcast.
    #[derive(Clone, Default)]
    struct Counts {
        prepared: Arc<AtomicUsize>,
        released: Arc<AtomicUsize>,
    }

    impl Counts {
        fn get(&self) -> (usize, usize) {
            (
                self.prepared.load(Ordering::Relaxed),
                self.released.load(Ordering::Relaxed),
            )
        }
    }

    struct CountingRecorder(Counts);

    impl Recorder for CountingRecorder {
        fn start(&mut self, _config: crate::recorder::RecordConfig) -> Result<(), RecorderError> {
            Ok(())
        }

        fn stop(&mut self) -> Result<crate::recorder::RecordingOutput, RecorderError> {
            Err(RecorderError::NotRecording)
        }

        fn is_recording(&self) -> bool {
            false
        }

        fn backend_name(&self) -> String {
            "counting".to_string()
        }

        fn prepare(&mut self) -> Result<(), RecorderError> {
            self.0.prepared.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn release(&mut self) {
            self.0.released.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn counting_supervisor() -> (Arc<Supervisor>, Counts) {
        let counts = Counts::default();
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(CountingRecorder(counts.clone()))));
        let db = Arc::new(Db::open_in_memory().unwrap());
        (
            Supervisor::new(recorder, std::env::temp_dir(), db),
            counts,
        )
    }

    #[test]
    fn capture_backend_is_released_at_idle() {
        let (sup, counts) = counting_supervisor();
        sup.sync_capture_backend(&GameState::Idle);
        assert_eq!(counts.get(), (0, 1), "idle should release, never prepare");
    }

    #[test]
    fn capture_backend_is_prepared_once_the_client_is_running() {
        let (sup, counts) = counting_supervisor();
        for state in [
            GameState::ClientRunning,
            GameState::WaitingForGame,
            GameState::Recording,
            GameState::Finalizing,
        ] {
            let before = counts.get();
            sup.sync_capture_backend(&state);
            let after = counts.get();
            assert_eq!(after.0, before.0 + 1, "{state:?} should prepare");
            assert_eq!(after.1, before.1, "{state:?} should not release");
        }
    }

    #[test]
    fn a_client_restart_does_not_churn_the_capture_backend() {
        // A restart emits StopGameflowWatch + StartGameflowWatch while
        // staying in ClientRunning. Syncing off the state — not the actions
        // — means nothing is torn down and brought straight back up.
        let (sup, counts) = counting_supervisor();
        sup.sync_capture_backend(&GameState::ClientRunning);
        sup.sync_capture_backend(&GameState::ClientRunning);
        assert_eq!(counts.get().1, 0, "no release across a restart");
    }

    fn test_supervisor() -> (Arc<Supervisor>, PathBuf) {
        // `line!()` used to stand in for a per-test discriminator here, but
        // it expands at this call site, not the caller's — so it's a single
        // constant and every test shared one directory. With tests running
        // in parallel, whichever finished first would `remove_dir_all` the
        // directory another was still recording into, failing that test's
        // `Recorder::stop` about 1 run in 5. A real counter keeps them apart.
        static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-supervisor-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let db = Arc::new(Db::open_in_memory().unwrap());
        (Supervisor::new(recorder, dir.clone(), db), dir)
    }

    #[test]
    fn stop_recording_writes_recording_and_markers_to_db() {
        let (sup, dir) = test_supervisor();

        sup.start_recording();
        assert!(sup.session.lock().unwrap().is_some());

        // Simulate a marker collected mid-recording — normally done by
        // `on_snapshot`, which needs a live poll to drive it.
        {
            let mut guard = sup.session.lock().unwrap();
            let session = guard.as_mut().unwrap();
            session.markers.push(SessionMarker {
                marker: Marker {
                    kind: MarkerKind::Kill,
                    game_time_s: 12.5,
                    payload: serde_json::json!({ "victim": "EnemyA" }),
                },
                video_time_s: 15.0,
            });
        }

        // Likewise a sample — normally pushed by `on_snapshot`.
        {
            let mut guard = sup.session.lock().unwrap();
            let session = guard.as_mut().unwrap();
            session.samples.push(SessionSample {
                game_time_s: 12.0,
                video_time_s: 14.5,
                diff: Some(live_client::TeamDiff {
                    our_team: "CHAOS".into(),
                    gold_diff_est: -1250.0,
                    kill_diff: -2,
                    cs_diff: 15,
                }),
                our_gold: 450.0,
                our_level: 11,
            });
        }

        sup.stop_recording();

        let finalized = sup
            .status()
            .last_finalized
            .expect("stop_recording should set last_finalized");
        assert!(finalized.recording_id.is_some(), "DB write should have succeeded");
        assert_eq!(finalized.markers.len(), 1);

        // Samples must land alongside the markers, signs intact — a
        // recording that finalizes without them renders a blank graph.
        let samples = sup.db.get_samples(finalized.recording_id.unwrap()).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].our_team, Some("CHAOS".to_string()));
        assert_eq!(samples[0].gold_diff_est, Some(-1250.0));
        assert_eq!(samples[0].kill_diff, Some(-2));

        let rows = sup.db.list_recordings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, finalized.recording_id.unwrap());

        assert!(
            sup.session.lock().unwrap().is_none(),
            "session should be cleared after stop"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_for_shutdown_is_a_no_op_with_nothing_recording() {
        let (sup, _dir) = test_supervisor();
        assert!(
            !sup.finalize_for_shutdown(),
            "nothing was recording, so there was nothing to finalize"
        );
        assert!(sup.status().last_finalized.is_none());
    }

    #[test]
    fn finalize_for_shutdown_writes_the_recording_so_quitting_cannot_lose_it() {
        let (sup, dir) = test_supervisor();
        sup.start_recording();
        assert!(sup.session.lock().unwrap().is_some());

        assert!(sup.finalize_for_shutdown(), "a recording was in flight");

        let finalized = sup
            .status()
            .last_finalized
            .expect("quitting mid-recording must still write the row");
        assert!(finalized.recording_id.is_some(), "DB write should have succeeded");
        assert!(sup.session.lock().unwrap().is_none(), "session should be cleared");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stop_recording_without_start_writes_nothing() {
        let (sup, dir) = test_supervisor();

        // StubRecorder::stop() without a prior start() errors — nothing
        // should reach the DB.
        sup.stop_recording();

        assert!(sup.status().last_finalized.is_none());
        assert!(sup.db.list_recordings().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }
}
