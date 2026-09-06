//! Synthetic library generation.
//!
//! Without this, the VOD library, its filters and sort, every retention
//! path, and the entire review player (timeline, marker clustering,
//! advantage curve) can only be exercised by finishing a real game on
//! Windows. On macOS the stub recorder writes a 31-byte placeholder that
//! no demuxer will open, so even a manual start/stop produces nothing
//! reviewable.
//!
//! Seeded recordings are real: a real file on disk in the recordings
//! directory, a real `recordings` row, real `markers` with the same
//! payload shapes `live_client::events::classify_event` produces, and a
//! real 1 Hz `samples` curve. `reconcile` and `retention` treat them
//! exactly like captured ones, which is the point — a seeded library that
//! reconcile would delete on next launch would test nothing.

use crate::{db, dev, AppState};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Filename prefix that marks a recording as seeded. Kept in the filename
/// rather than a DB column so it survives a `reconcile` re-import and
/// needs no migration.
const SEED_PREFIX: &str = "seed-";

const CHAMPIONS: &[&str] = &[
    "Ahri", "Darius", "Ezreal", "Jinx", "Lee Sin", "Lux", "Nautilus", "Orianna", "Riven",
    "Sett", "Thresh", "Vayne", "Viktor", "Yasuo", "Zed",
];
const ROLES: &[&str] = &["TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY"];
const QUEUES: &[i64] = &[400, 420, 430, 440, 450, 700];
const ENEMIES: &[&str] = &[
    "Sylas", "Kai'Sa", "Renekton", "Nami", "Viego", "Kayle", "Twitch", "Rell",
];
const DRAGONS: &[&str] = &["Infernal", "Ocean", "Mountain", "Cloud", "Hextech", "Chemtech"];

#[derive(Debug, Clone, Deserialize)]
pub struct SeedSpec {
    pub count: usize,
    pub duration_min_s: f64,
    pub duration_max_s: f64,
    pub markers_min: usize,
    pub markers_max: usize,
    /// Generate the 1 Hz advantage curve behind the review timeline's graph.
    pub samples: bool,
    /// Size of the media file written. Retention is a size-based policy,
    /// so testing it needs files with plausible sizes — 3 GiB here costs
    /// nothing thanks to sparse allocation (see `write_media_file`).
    pub file_bytes: u64,
    /// Copy `fixtures/sample.mp4` instead of writing a placeholder, when
    /// it exists. The only way to get a genuinely playable seeded VOD.
    pub use_sample_mp4: bool,
    /// Back-date `started_at` across this many days, so a max-age policy
    /// has something to bite on.
    pub spread_days: i64,
    /// Pin every Nth recording (0 = none), to prove pinned rows are exempt
    /// from deletion while still counting toward the total.
    pub pinned_every: usize,
    /// Mix in NULL metadata, unicode, and long names — the states the
    /// library UI actually has to survive.
    pub messy: bool,
    pub seed: u64,
}

impl Default for SeedSpec {
    fn default() -> Self {
        Self {
            count: 10,
            duration_min_s: 900.0,
            duration_max_s: 2400.0,
            markers_min: 8,
            markers_max: 30,
            samples: true,
            file_bytes: 64 * 1024,
            use_sample_mp4: false,
            spread_days: 14,
            pinned_every: 0,
            messy: false,
            seed: 1,
        }
    }
}

#[derive(Serialize)]
pub struct SeedReport {
    pub recording_ids: Vec<i64>,
    pub markers_inserted: usize,
    pub samples_inserted: usize,
    pub bytes_written: u64,
    pub used_sample_mp4: bool,
    pub paths: Vec<String>,
}

#[tauri::command]
pub fn dev_seed_library(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    spec: SeedSpec,
) -> Result<SeedReport, String> {
    std::fs::create_dir_all(&state.recordings_dir).map_err(|e| e.to_string())?;

    let sample_mp4 = spec
        .use_sample_mp4
        .then(sample_mp4_path)
        .flatten();
    let mut rng = Rng::new(spec.seed);
    let now = now_millis();

    let mut report = SeedReport {
        recording_ids: Vec::new(),
        markers_inserted: 0,
        samples_inserted: 0,
        bytes_written: 0,
        used_sample_mp4: sample_mp4.is_some(),
        paths: Vec::new(),
    };

    for i in 0..spec.count {
        let plan = plan_recording(&spec, &mut rng, i, now);
        let path = state
            .recordings_dir
            .join(format!("{SEED_PREFIX}{}-{}.mp4", spec.seed, plan.stamp));

        let written = write_media_file(&path, spec.file_bytes, sample_mp4.as_deref())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        report.bytes_written += written;

        let id = state
            .db
            .insert_recording(&db::NewRecording {
                path: path.display().to_string(),
                started_at: plan.started_at,
                duration_s: Some(plan.duration_s),
                game_id: Some(plan.game_id),
                queue: plan.queue,
                champion: plan.champion.clone(),
                role: plan.role.clone(),
                win: plan.win,
                kda_k: plan.kda.map(|k| k.0),
                kda_d: plan.kda.map(|k| k.1),
                kda_a: plan.kda.map(|k| k.2),
                patch: Some("15.17".to_string()),
                pinned: plan.pinned,
                // The row must agree with the file, or the very first
                // retention pass computes a total that doesn't match disk.
                size_bytes: written as i64,
                // Alternate layouts so the review player's stem picker has
                // something to render against seeded rows — including the
                // single-track case, where it must not appear at all. The
                // seeded media has no audio (fixtures/README.md), so
                // selecting a stem still fails; this exercises the UI, not
                // playback. See docs/dev-portal.md's known limits.
                audio_tracks_json: serde_json::to_string(&audio_layout_for(i)).ok(),
            })
            .map_err(|e| e.to_string())?;

        let markers = plan_markers(&spec, &mut rng, plan.duration_s);
        state
            .db
            .insert_markers(id, &markers)
            .map_err(|e| e.to_string())?;
        report.markers_inserted += markers.len();

        if spec.samples {
            let samples = plan_samples(&mut rng, plan.duration_s, &markers);
            state
                .db
                .insert_samples(id, &samples)
                .map_err(|e| e.to_string())?;
            report.samples_inserted += samples.len();
        }

        report.recording_ids.push(id);
        report.paths.push(path.display().to_string());
    }

    dev::notify_library_changed(&app);
    Ok(report)
}

#[derive(Serialize)]
pub struct ClearReport {
    pub rows_deleted: usize,
    pub files_deleted: usize,
}

/// Removes only what this module created — rows whose file basename starts
/// with `seed-`, and those files. Real captured recordings are untouched.
#[tauri::command]
pub fn dev_clear_seeded(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
) -> Result<ClearReport, String> {
    let rows = state.db.list_recordings().map_err(|e| e.to_string())?;
    let mut report = ClearReport {
        rows_deleted: 0,
        files_deleted: 0,
    };

    for row in rows.iter().filter(|r| is_seeded(&r.path)) {
        if std::fs::remove_file(&row.path).is_ok() {
            report.files_deleted += 1;
        }
        state
            .db
            .delete_recording(row.id)
            .map_err(|e| e.to_string())?;
        report.rows_deleted += 1;
    }

    // Seeded files with no row (a manual DB wipe, or a failed insert) are
    // still ours to clean up — otherwise the next reconcile re-imports
    // them as untracked recordings.
    if let Ok(entries) = std::fs::read_dir(&state.recordings_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_seeded(&path.display().to_string()) && std::fs::remove_file(&path).is_ok() {
                report.files_deleted += 1;
            }
        }
    }

    dev::notify_library_changed(&app);
    Ok(report)
}

fn is_seeded(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(SEED_PREFIX))
}

fn sample_mp4_path() -> Option<PathBuf> {
    let path = super::info::repo_fixtures_dir()?.join("sample.mp4");
    path.exists().then_some(path)
}

/// Writes the backing media file. When no real clip is available this
/// creates a *sparse* file via `set_len` — a 3 GiB retention fixture then
/// costs a few hundred bytes of actual disk while still reporting 3 GiB to
/// `metadata().len()`, which is what both the DB row and retention read.
/// The file will not decode, and is not meant to: use `use_sample_mp4`
/// for anything that has to play.
fn write_media_file(path: &Path, bytes: u64, sample: Option<&Path>) -> std::io::Result<u64> {
    if let Some(sample) = sample {
        std::fs::copy(sample, path)?;
        return Ok(std::fs::metadata(path)?.len());
    }

    let mut file = std::fs::File::create(path)?;
    // A recognisable header, so a stray seeded file found later on disk
    // explains itself.
    file.write_all(b"NINJA_RECORDER_SEEDED_PLACEHOLDER\n")?;
    let header = 34u64;
    if bytes > header {
        file.set_len(bytes)?;
    }
    file.sync_all()?;
    Ok(std::fs::metadata(path)?.len())
}

struct RecordingPlan {
    stamp: i64,
    started_at: i64,
    duration_s: f64,
    game_id: i64,
    queue: Option<i64>,
    champion: Option<String>,
    role: Option<String>,
    win: Option<bool>,
    kda: Option<(i64, i64, i64)>,
    pinned: bool,
}

/// Cycles the seeded rows through the real presets so the library contains
/// a mix of single-track and multi-track recordings.
fn audio_layout_for(index: usize) -> crate::recorder::audio::AudioLayout {
    use crate::recorder::audio::AudioPreset;
    match index % 3 {
        0 => AudioPreset::Game,
        1 => AudioPreset::GameMic { mic_device_id: None },
        _ => AudioPreset::GameMicDiscord { mic_device_id: None },
    }
    .layout()
}

fn plan_recording(spec: &SeedSpec, rng: &mut Rng, index: usize, now: i64) -> RecordingPlan {
    let duration_s = rng.range_f64(spec.duration_min_s, spec.duration_max_s.max(spec.duration_min_s));
    // Spread backwards from now, oldest last, so a max-age policy has a
    // predictable set of victims.
    let day_span_ms = spec.spread_days.max(0) * 24 * 60 * 60 * 1000;
    let age = if spec.count > 1 && day_span_ms > 0 {
        day_span_ms * index as i64 / (spec.count as i64 - 1)
    } else {
        0
    };
    let started_at = now - age - rng.range_i64(0, 3_600_000);

    // Every third recording in messy mode has unknown metadata — the NULL
    // state a real row sits in until `fetch_match_summary` is wired up
    // (DEVELOPMENT.md §3.4), which the library UI must render without
    // falling apart.
    let unknown = spec.messy && index % 3 == 2;

    RecordingPlan {
        stamp: started_at,
        started_at,
        duration_s,
        game_id: 5_000_000_000 + index as i64,
        queue: (!unknown).then(|| *rng.pick(QUEUES)),
        champion: if unknown {
            None
        } else if spec.messy && index % 5 == 1 {
            // Unicode and an over-long name, to exercise escaping and the
            // library row's layout.
            Some("Kha'Zix ✦ 日本語テスト ✦ a-very-long-champion-name".to_string())
        } else {
            Some(rng.pick(CHAMPIONS).to_string())
        },
        role: (!unknown).then(|| rng.pick(ROLES).to_string()),
        win: if unknown { None } else { Some(rng.next_bool()) },
        kda: (!unknown).then(|| {
            (
                rng.range_i64(0, 18),
                rng.range_i64(0, 12),
                rng.range_i64(0, 25),
            )
        }),
        pinned: spec.pinned_every > 0 && index.is_multiple_of(spec.pinned_every),
    }
}

/// Markers with the exact payload shapes `classify_event` emits, so
/// `markerLabel()` in `review.ts` renders real prose instead of its "?"
/// fallback, and so the timeline's kind-priority clustering has a
/// realistic mix to cluster.
fn plan_markers(spec: &SeedSpec, rng: &mut Rng, duration_s: f64) -> Vec<db::NewMarker> {
    let count = rng.range_usize(spec.markers_min, spec.markers_max.max(spec.markers_min));
    let mut markers = Vec::with_capacity(count + 1);

    // Recording starts on the loading screen, so video time runs a little
    // ahead of game time — the same offset `TimeAlignment` produces.
    let offset_s = 12.0;
    let mk = |kind: &str, game_time_s: f64, payload: serde_json::Value| db::NewMarker {
        game_time_s,
        video_time_s: (game_time_s + offset_s).max(0.0),
        kind: kind.to_string(),
        payload_json: payload.to_string(),
    };

    if count > 0 {
        markers.push(mk(
            "first_blood",
            rng.range_f64(60.0, 300.0),
            serde_json::json!({ "recipient": rng.pick(CHAMPIONS) }),
        ));
    }

    for _ in 0..count {
        // Bias toward mid/late game, and deliberately allow near-identical
        // timestamps: teamfight pile-ups are what the timeline's pixel
        // clustering exists for.
        let t = rng.range_f64(30.0, (duration_s - 10.0).max(60.0));
        let marker = match rng.range_usize(0, 9) {
            0..=2 => mk("kill", t, serde_json::json!({ "victim": rng.pick(ENEMIES) })),
            3..=4 => mk("death", t, serde_json::json!({ "killer": rng.pick(ENEMIES) })),
            5 => mk(
                "assist",
                t,
                serde_json::json!({ "victim": rng.pick(ENEMIES), "killer": rng.pick(CHAMPIONS) }),
            ),
            6 => mk(
                "dragon",
                t,
                serde_json::json!({ "killer": rng.pick(CHAMPIONS), "dragon_type": rng.pick(DRAGONS) }),
            ),
            7 => mk(
                "turret",
                t,
                serde_json::json!({ "killer": rng.pick(CHAMPIONS), "turret": "Turret_T1_C_05_A" }),
            ),
            8 => mk("baron", t, serde_json::json!({ "killer": rng.pick(CHAMPIONS) })),
            _ => mk(
                "ace",
                t,
                serde_json::json!({ "acer": rng.pick(CHAMPIONS), "acing_team": "ORDER" }),
            ),
        };
        markers.push(marker);
    }

    markers.sort_by(|a, b| a.video_time_s.total_cmp(&b.video_time_s));
    markers
}

/// A 1 Hz advantage curve that crosses zero and reacts to markers, so the
/// review graph's signed ahead/behind fills, its symmetric-about-zero
/// scaling, and its max-abs downsampling all get something real to render
/// rather than a straight line.
fn plan_samples(rng: &mut Rng, duration_s: f64, markers: &[db::NewMarker]) -> Vec<db::NewSample> {
    let seconds = duration_s.max(1.0) as i64;
    let mut samples = Vec::with_capacity(seconds as usize);

    let mut gold = 0.0f64;
    let mut kills = 0i64;
    let mut cs = 0i64;
    // Random-walk drift, re-rolled occasionally so the curve has phases
    // (a losing early game that turns around) rather than one trend.
    let mut drift = rng.range_f64(-8.0, 8.0);

    for t in 0..seconds {
        let game_time_s = t as f64;
        if t % 120 == 0 {
            drift = rng.range_f64(-10.0, 10.0);
        }
        gold += drift + rng.range_f64(-25.0, 25.0);
        cs += rng.range_i64(-1, 2);

        // Kills and deaths in the marker stream move the kill diff, so the
        // graph and the glyphs above it tell the same story.
        for m in markers.iter().filter(|m| m.game_time_s.floor() as i64 == t) {
            match m.kind.as_str() {
                "kill" => {
                    kills += 1;
                    gold += 300.0;
                }
                "death" => {
                    kills -= 1;
                    gold -= 300.0;
                }
                _ => {}
            }
        }

        samples.push(db::NewSample {
            game_time_s,
            video_time_s: game_time_s + 12.0,
            our_team: Some("ORDER".to_string()),
            gold_diff_est: Some(gold),
            kill_diff: Some(kills),
            cs_diff: Some(cs),
            our_gold: Some(rng.range_f64(0.0, 2500.0)),
            our_level: Some((1 + t / 90).min(18)),
        });
    }
    samples
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// SplitMix64. Deterministic for a given seed, which is what makes a
/// seeded library reproducible — "seed 7 then look at recording 3" has to
/// mean the same thing twice. Inlined rather than pulling in `rand`: this
/// is the only randomness in the crate and it is dev-only.
pub(crate) struct Rng {
    state: u64,
}

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            // A zero seed would otherwise produce a zero stream forever.
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xDEAD_BEEF_CAFE_F00D,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        // Top 53 bits — the exactly-representable range for an f64 mantissa.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        if hi <= lo {
            return lo;
        }
        lo + self.next_f64() * (hi - lo)
    }

    /// Inclusive of `lo`, exclusive of `hi`, clamped when the range is empty.
    fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo) as u64) as i64
    }

    fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo) as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_produces_the_same_stream() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn zero_seed_is_not_a_constant_stream() {
        let mut rng = Rng::new(0);
        let first = rng.next_u64();
        assert!((0..16).any(|_| rng.next_u64() != first));
    }

    #[test]
    fn ranges_stay_in_bounds_and_tolerate_empty_ones() {
        let mut rng = Rng::new(7);
        for _ in 0..500 {
            let f = rng.range_f64(-3.0, 9.5);
            assert!((-3.0..9.5).contains(&f));
            let i = rng.range_i64(2, 5);
            assert!((2..5).contains(&i));
        }
        assert_eq!(rng.range_i64(4, 4), 4);
        assert_eq!(rng.range_f64(1.0, 0.0), 1.0);
        assert_eq!(rng.range_usize(9, 3), 9);
    }

    #[test]
    fn seeded_markers_are_deterministic_and_ordered() {
        let spec = SeedSpec::default();
        let render = |seed: u64| {
            let mut rng = Rng::new(seed);
            plan_markers(&spec, &mut rng, 1800.0)
        };
        let a = render(11);
        let b = render(11);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.kind, y.kind);
            assert_eq!(x.payload_json, y.payload_json);
            assert_eq!(x.video_time_s, y.video_time_s);
        }
        assert!(a.windows(2).all(|w| w[0].video_time_s <= w[1].video_time_s));
    }

    #[test]
    fn seeded_marker_payloads_match_the_keys_the_review_ui_reads() {
        let spec = SeedSpec {
            markers_min: 200,
            markers_max: 200,
            ..SeedSpec::default()
        };
        let mut rng = Rng::new(3);
        for marker in plan_markers(&spec, &mut rng, 1800.0) {
            let payload: serde_json::Value = serde_json::from_str(&marker.payload_json).unwrap();
            let required: &[&str] = match marker.kind.as_str() {
                "kill" => &["victim"],
                "death" => &["killer"],
                "assist" => &["victim", "killer"],
                "dragon" => &["killer", "dragon_type"],
                "turret" => &["killer", "turret"],
                "baron" | "herald" => &["killer"],
                "ace" => &["acer", "acing_team"],
                "first_blood" => &["recipient"],
                other => panic!("unexpected seeded marker kind {other}"),
            };
            for key in required {
                assert!(
                    payload.get(key).and_then(|v| v.as_str()).is_some(),
                    "{} marker is missing a string `{key}`: {payload}",
                    marker.kind
                );
            }
        }
    }

    #[test]
    fn samples_are_one_per_second_and_react_to_kill_markers() {
        let mut rng = Rng::new(5);
        let markers = vec![db::NewMarker {
            game_time_s: 10.0,
            video_time_s: 22.0,
            kind: "kill".to_string(),
            payload_json: "{}".to_string(),
        }];
        let samples = plan_samples(&mut rng, 60.0, &markers);
        assert_eq!(samples.len(), 60);
        assert_eq!(samples[9].kill_diff, Some(0));
        assert_eq!(samples[10].kill_diff, Some(1));
        assert!(samples.windows(2).all(|w| w[1].game_time_s > w[0].game_time_s));
    }

    #[test]
    fn spread_days_back_dates_recordings_oldest_last() {
        let spec = SeedSpec {
            count: 5,
            spread_days: 10,
            ..SeedSpec::default()
        };
        let now = 1_700_000_000_000;
        let mut rng = Rng::new(1);
        let ages: Vec<i64> = (0..spec.count)
            .map(|i| now - plan_recording(&spec, &mut rng, i, now).started_at)
            .collect();
        assert!(ages.windows(2).all(|w| w[1] > w[0]), "{ages:?}");
        assert!(*ages.last().unwrap() >= 10 * 24 * 60 * 60 * 1000);
    }

    #[test]
    fn seeded_paths_are_recognised_and_real_ones_are_not() {
        assert!(is_seeded("/tmp/recordings/seed-1-1700000000000.mp4"));
        assert!(!is_seeded("/tmp/recordings/recording-1700000000000.mp4"));
    }
}
