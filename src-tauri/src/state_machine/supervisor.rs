//! Async orchestration around the pure `StateMachine`: spawns/aborts the
//! lockfile, gameflow, and Live Client Data watchers per `Action`, and
//! drives the `Recorder` (Phase 1) and marker pipeline (`live_client`).
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
}

struct RecordingSession {
    tracker: MarkerTracker,
    markers: Vec<SessionMarker>,
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
        })
    }

    pub fn status(&self) -> SupervisorStatus {
        SupervisorStatus {
            state: self.machine.lock().unwrap().state.clone(),
            last_finalized: self.last_finalized.lock().unwrap().clone(),
        }
    }

    /// Starts the always-on lockfile watch. Everything else (gameflow
    /// watch, Live Client Data polling, recording) is started/stopped by
    /// state transitions from here on. Runs for the app's lifetime.
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
        let actions = {
            let mut machine = self.machine.lock().unwrap();
            machine.handle(event)
        };
        for action in actions {
            self.execute(action);
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
        let started_at_millis = timestamp_millis();
        let config = RecordConfig {
            output_dir: self.recordings_dir.clone(),
            file_stem: format!("recording-{started_at_millis}"),
        };
        match self.recorder.lock().unwrap().start(config) {
            Ok(()) => {
                *self.session.lock().unwrap() = Some(RecordingSession {
                    tracker: MarkerTracker::new(),
                    markers: Vec::new(),
                    alignment: None,
                    record_started_at: Instant::now(),
                    started_at_millis,
                });
            }
            Err(e) => eprintln!("[state_machine] failed to start recording: {e}"),
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
            Ok(path) => {
                let markers = session.as_ref().map(|s| s.markers.clone()).unwrap_or_default();
                let started_at = session
                    .as_ref()
                    .map(|s| s.started_at_millis)
                    .unwrap_or_else(timestamp_millis);
                let path_str = path.display().to_string();
                let size_bytes = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);

                let recording_id = match self.db.insert_recording(&db::NewRecording {
                    path: path_str.clone(),
                    started_at,
                    size_bytes,
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
            }
            Err(e) => eprintln!("[state_machine] failed to stop recording: {e}"),
        }
    }
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

    fn test_supervisor() -> (Arc<Supervisor>, PathBuf) {
        // `line!()` here would be constant across every call site (it's
        // evaluated where the macro appears, not where the function is
        // called), so every test sharing this helper would get the exact
        // same temp dir — since the test harness runs tests in parallel,
        // one test's `remove_dir_all` teardown could then delete the
        // directory out from under another test still recording. A
        // per-call counter keeps each test's directory unique.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let recorder: Arc<Mutex<Box<dyn Recorder>>> =
            Arc::new(Mutex::new(Box::new(StubRecorder::new())));
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ninja-recorder-supervisor-test-{}-{}",
            std::process::id(),
            n
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

        sup.stop_recording();

        let finalized = sup
            .status()
            .last_finalized
            .expect("stop_recording should set last_finalized");
        assert!(finalized.recording_id.is_some(), "DB write should have succeeded");
        assert_eq!(finalized.markers.len(), 1);

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
