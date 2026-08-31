# fixtures

Sample data used so the app is developable without League running or a real
recording backend.

- `sample.mp4` (not checked in yet) — a short real clip. When present,
  `StubRecorder` (`src-tauri/src/recorder/stub.rs`) copies it as the output
  of every stub-recorded "game" instead of writing a placeholder file, so
  the review player (Phase 5) has something real to seek/play. Keep it
  small (a few MB, a few seconds) — this is a fixture, not a demo asset.
- LCU / Live Client Data JSON fixtures land here in Phase 2/3 — every
  response shape the app depends on gets captured here the first time it's
  seen, per DEVELOPMENT.md §3.3.
