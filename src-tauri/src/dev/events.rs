//! Recording the LCU's event firehose, raw. #149.
//!
//! **Raw and unfiltered, on purpose.** A URI filter presupposes knowing which
//! endpoint carries the thing you are hunting, and the case this was built for
//! is exactly the opposite: the end-of-game block turned out to carry no LP
//! change, neither ranked endpoint reports one, and the client plainly knows it
//! because it animates it on the post-game screen. Something says so; nothing
//! says what. So this writes every frame and sorts it out afterwards —
//! `event_uris` over a finished capture is how you find the needle.
//!
//! **A second socket, not a tap on the gameflow one.** That watch's lifetime
//! belongs to the state machine: it comes up with the lockfile and goes away
//! with it, and threading a debug recorder through it would couple a
//! throwaway tool to the path that decides when recordings start. A second
//! connection to a process on localhost costs nothing and can be armed and
//! disarmed on its own.
//!
//! **Bounded, and it stops rather than rotating.** A rotation would discard
//! the beginning of a session to keep recording, and for a probe the beginning
//! is usually the part being looked for. Hitting the cap stops the capture and
//! says so, which is a result; a file whose middle is missing is not.

use crate::{info, warn};
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;

/// How much one capture may write before it stops itself.
///
/// The firehose is every endpoint the client touches, continuously, so this is
/// a real limit rather than a formality — but a post-game window is seconds,
/// not hours, and 64 MiB is far more than one needs while still being a size a
/// text editor will open.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

struct Capture {
    path: PathBuf,
    stop: std::sync::Arc<AtomicBool>,
    frames: std::sync::Arc<AtomicU64>,
    bytes: std::sync::Arc<AtomicU64>,
}

fn slot() -> &'static Mutex<Option<Capture>> {
    static SLOT: OnceLock<Mutex<Option<Capture>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// What a capture is doing, or last did.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureStatus {
    pub recording: bool,
    pub path: Option<String>,
    pub frames: u64,
    pub bytes: u64,
    pub max_bytes: u64,
}

fn status_locked(held: &Option<Capture>) -> CaptureStatus {
    match held {
        Some(c) => CaptureStatus {
            recording: !c.stop.load(Ordering::Relaxed),
            path: Some(c.path.display().to_string()),
            frames: c.frames.load(Ordering::Relaxed),
            bytes: c.bytes.load(Ordering::Relaxed),
            max_bytes: MAX_BYTES,
        },
        None => CaptureStatus {
            recording: false,
            path: None,
            frames: 0,
            bytes: 0,
            max_bytes: MAX_BYTES,
        },
    }
}

/// Starts recording every LCU event to a JSONL file.
///
/// One JSON object per line, so the file is greppable while it is still being
/// written and survives being cut off — a single top-level array would have to
/// be complete before anything could parse it, which is the wrong property for
/// a capture that may end when a cap is hit or a client quits.
#[tauri::command]
pub async fn dev_event_capture_start() -> Result<CaptureStatus, String> {
    {
        let held = slot().lock().unwrap();
        if held.as_ref().is_some_and(|c| !c.stop.load(Ordering::Relaxed)) {
            return Err("a capture is already running; stop it first".into());
        }
    }

    let lockfile = crate::lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running (no lockfile found)".to_string())?;

    let dir = crate::fixtures::base_dir()
        .ok_or_else(|| "fixtures directory not initialized".to_string())?
        .join("lcu-events");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Named by start time so captures never overwrite each other — unlike the
    // per-endpoint fixtures, where the latest response is the interesting one.
    let path = dir.join(format!(
        "events-{}.jsonl",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let frames = std::sync::Arc::new(AtomicU64::new(0));
    let bytes = std::sync::Arc::new(AtomicU64::new(0));

    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;

    let (task_stop, task_frames, task_bytes) = (stop.clone(), frames.clone(), bytes.clone());
    let task_path = path.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = pump(&lockfile, &mut file, &task_stop, &task_frames, &task_bytes).await {
            warn!("dev-events", "capture ended: {e}");
        }
        task_stop.store(true, Ordering::Relaxed);
        let _ = file.flush();
        info!("dev-events", "wrote {} frames to {}",
            task_frames.load(Ordering::Relaxed), task_path.display()
        );
    });

    let capture = Capture { path, stop, frames, bytes };
    let status = status_locked(&Some(Capture {
        path: capture.path.clone(),
        stop: capture.stop.clone(),
        frames: capture.frames.clone(),
        bytes: capture.bytes.clone(),
    }));
    *slot().lock().unwrap() = Some(capture);
    Ok(status)
}

/// The socket loop. Every text frame is written verbatim, with a timestamp.
async fn pump(
    lockfile: &crate::lcu::LockfileInfo,
    file: &mut std::fs::File,
    stop: &AtomicBool,
    frames: &AtomicU64,
    bytes: &AtomicU64,
) -> Result<(), String> {
    let mut request = lockfile.ws_url().into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        AUTHORIZATION,
        crate::lcu::client::basic_auth_header(&lockfile.password)
            .parse()
            .map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue| e.to_string())?,
    );

    // The client's certificate is self-signed and per-install; the gameflow
    // watch accepts it the same way, and the endpoint is a loopback socket
    // whose password came from a file only this user can read.
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;

    let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(tokio_tungstenite::Connector::NativeTls(connector)),
    )
    .await
    .map_err(|e| e.to_string())?;

    // The same WAMP-lite subscribe the gameflow watch uses, and the same
    // firehose — the difference is that nothing is filtered out here.
    ws.send(Message::Text(
        serde_json::to_string(&(5, "OnJsonApiEvent")).map_err(|e| e.to_string())?,
    ))
    .await
    .map_err(|e| e.to_string())?;

    use futures_util::{SinkExt, StreamExt};
    while let Some(message) = ws.next().await {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(Message::Text(text)) = message else {
            continue;
        };
        // The subscribe acknowledgement and any keepalive arrive as frames
        // too; they are written like everything else, because deciding what
        // is uninteresting is the judgement this tool exists to avoid making.
        let line = format!(
            "{{\"at\":{},\"frame\":{}}}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            if text.trim().is_empty() { "null" } else { text.trim() }
        );
        if file.write_all(line.as_bytes()).is_err() {
            break;
        }
        frames.fetch_add(1, Ordering::Relaxed);
        if bytes.fetch_add(line.len() as u64, Ordering::Relaxed) + (line.len() as u64) >= MAX_BYTES {
            warn!("dev-events", "hit the {MAX_BYTES}-byte cap; stopping");
            break;
        }
    }
    let _ = ws.close(None).await;
    Ok(())
}

/// Stops the running capture, if there is one.
#[tauri::command]
pub fn dev_event_capture_stop() -> CaptureStatus {
    let held = slot().lock().unwrap();
    if let Some(c) = held.as_ref() {
        c.stop.store(true, Ordering::Relaxed);
    }
    status_locked(&held)
}

/// Whether a capture is running, and how much it has written.
#[tauri::command]
pub fn dev_event_capture_status() -> CaptureStatus {
    let held = slot().lock().unwrap();
    status_locked(&held)
}

/// One URI seen in a capture, and how often.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UriCount {
    pub uri: String,
    pub count: usize,
    /// The last `eventType` seen for it — `Create`, `Update`, `Delete`.
    pub event_type: Option<String>,
}

/// Which endpoints appeared in a capture, most frequent first.
///
/// **This is the half that makes a raw capture usable.** A post-game window is
/// thousands of frames across dozens of endpoints, and the question is always
/// "which of these could possibly carry the thing I am looking for" — a
/// question a list of URIs answers in seconds and a 40 MB file does not.
pub fn event_uris(lines: &str) -> Vec<UriCount> {
    let mut counts: std::collections::BTreeMap<String, (usize, Option<String>)> =
        std::collections::BTreeMap::new();

    for line in lines.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // `[8, "OnJsonApiEvent", {uri, eventType, data}]`, wrapped by the
        // writer in `{at, frame}`.
        let Some(event) = value.get("frame").and_then(|f| f.as_array()).and_then(|a| a.get(2))
        else {
            continue;
        };
        let Some(uri) = event.get("uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let entry = counts.entry(uri.to_string()).or_insert((0, None));
        entry.0 += 1;
        if let Some(kind) = event.get("eventType").and_then(|e| e.as_str()) {
            entry.1 = Some(kind.to_string());
        }
    }

    let mut found: Vec<UriCount> = counts
        .into_iter()
        .map(|(uri, (count, event_type))| UriCount { uri, count, event_type })
        .collect();
    found.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.uri.cmp(&b.uri)));
    found
}

/// `event_uris` over a capture file.
#[tauri::command]
pub fn dev_event_uris(path: String) -> Result<Vec<UriCount>, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    Ok(event_uris(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(uri: &str, kind: &str) -> String {
        format!(
            r#"{{"at":1,"frame":[8,"OnJsonApiEvent",{{"uri":"{uri}","eventType":"{kind}","data":{{}}}}]}}"#
        )
    }

    #[test]
    fn uris_are_counted_and_the_loudest_leads() {
        let capture = [
            line("/lol-gameflow/v1/gameflow-phase", "Update"),
            line("/lol-ranked/v1/notifications", "Create"),
            line("/lol-gameflow/v1/gameflow-phase", "Update"),
            line("/lol-gameflow/v1/gameflow-phase", "Update"),
        ]
        .join("\n");

        let found = event_uris(&capture);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].uri, "/lol-gameflow/v1/gameflow-phase");
        assert_eq!(found[0].count, 3);
        assert_eq!(found[0].event_type.as_deref(), Some("Update"));
        assert_eq!(found[1].uri, "/lol-ranked/v1/notifications");
    }

    /// A capture can end mid-line when a cap is hit or the client quits, and
    /// the summary has to survive that rather than refusing the whole file.
    #[test]
    fn a_truncated_last_line_does_not_lose_the_rest() {
        let capture = format!(
            "{}\n{}\n{{\"at\":9,\"frame\":[8,\"OnJsonApi",
            line("/lol-ranked/v1/notifications", "Create"),
            line("/lol-ranked/v1/notifications", "Create"),
        );
        let found = event_uris(&capture);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].count, 2);
    }

    /// Frames that are not events — the subscribe acknowledgement, keepalives
    /// — are written but must not become phantom URIs.
    #[test]
    fn non_event_frames_are_skipped_in_the_summary() {
        let capture = [
            r#"{"at":1,"frame":[]}"#.to_string(),
            r#"{"at":2,"frame":null}"#.to_string(),
            line("/lol-ranked/v1/notifications", "Create"),
        ]
        .join("\n");
        assert_eq!(event_uris(&capture).len(), 1);
    }

    #[test]
    fn an_empty_capture_is_not_an_error() {
        assert_eq!(event_uris(""), vec![]);
    }
}
