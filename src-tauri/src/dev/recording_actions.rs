//! The things you can do to one recording, from the inspector (#99).
//!
//! **Deliberately not in `recording_api`.** That module is a pure read and
//! says so: opening the inspector can never change what it describes. These
//! change things, so they live apart — the split is the property, not a
//! filing convention. A reader of either module can tell which half they are
//! in without checking every function.
//!
//! Nothing here is a new answer to a question something else already answers.
//! Two of the four actions the inspector offers are existing commands called
//! with the row's own values (`dev_patch_match_summary`, `dev_trim_lead_in`);
//! the backfill goes through the same candidate query and the same loop the
//! bulk run uses. What was missing was a way to point them at the recording
//! in front of you.

use crate::AppState;
use crate::backfill::BackfillReport;

/// Runs the backfill against one recording.
///
/// The bulk run is in Settings and works on the whole library, which is the
/// wrong shape when the question is about a single row. This is the same
/// pass: same candidate query, same matching, same refusal when more than one
/// game overlaps.
///
/// A recording the backfill has nothing to fill comes back with `scanned: 0`
/// rather than an error — "there was nothing to do" is an answer, and it is
/// often the one being checked for.
#[tauri::command]
pub async fn dev_backfill_recording(
    state: tauri::State<'_, AppState>,
    recording_id: i64,
) -> Result<BackfillReport, String> {
    let db = state.db.clone();
    crate::backfill::run_one(&db, recording_id).await
}

/// Which of the two file actions to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reveal {
    /// Hand the file to whatever plays it.
    Play,
    /// Show it in the OS file manager, selected.
    Folder,
}

/// Opens a recording's file, or shows it in the file manager.
///
/// Takes a recording id rather than a path: the path comes out of the row, so
/// nothing the frontend holds decides which file is opened. `reconcile`
/// imports whatever video files it finds, and a path is the one field on a
/// row that is not ours.
///
/// **Refuses a file that is not there**, rather than handing a missing path to
/// the OS and getting whatever error it feels like showing. A row whose file
/// has been deleted is a real state — `reconcile` cleans those up on scan, but
/// only when it runs.
#[tauri::command]
pub fn dev_reveal_recording(
    state: tauri::State<AppState>,
    recording_id: i64,
    which: Reveal,
) -> Result<(), String> {
    let row = state
        .db
        .get_recording(recording_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no recording {recording_id}"))?;

    let path = std::path::Path::new(&row.path);
    if !path.exists() {
        return Err(format!("the file is gone: {}", row.path));
    }

    match which {
        Reveal::Play => {
            tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
        }
        Reveal::Folder => {
            tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
        }
    }
}
