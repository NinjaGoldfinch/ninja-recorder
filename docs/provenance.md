# Provenance

Where this repository's code came from, and what is still owed because of it.

v2 is not a rewrite. This repository starts as a verbatim copy of v1 and
changes the frontend, the IPC contract, the process model, the quality gates
and the capture backend in place, one workstream at a time.

**The two repositories have swapped names.** v1 was
[`ninja-recorder`](https://github.com/NinjaGoldfinch/ninja-recorder-deprecated)
and is now `ninja-recorder-deprecated`, archived and kept for history; this
repository was `ninja-recorder-v2` and now holds the plain name. Where the
text below says "v1" it means the deprecated repository, whatever either of
them was called at the time. This file records what was copied, from where, and under what licence, so
that the WS8 licence exit (see the plan's §8) is an audit of a known list rather
than an archaeology exercise.

## Source

| Field | Value |
|---|---|
| Upstream | `github.com/NinjaGoldfinch/ninja-recorder` |
| Branch | `main` |
| Commit | `32dcd4195b2d14730e3ff4cd98443ee710fb1815` (`32dcd41`) |
| Commit date | 2026-09-12 |
| Nearest tag | `v1.1.0-alpha.67` |
| App version at that commit | `0.8.0` in `src-tauri/Cargo.toml` and `tauri.conf.json`; `1.1.0` declared in `package.json` |
| Licence | GPL-2.0-only |

The copy was taken with `git archive 32dcd41 | tar -x`, so every path below is
byte-identical to upstream at that commit as of the import commit. Later
commits in this repository change some of them; `git log --follow <path>` from
the import commit is the diff that matters.

## Licence

`LICENSE` (GPL-2.0) is carried over unchanged. **Relicensing is WS8 and happens
only after libobs is gone**. See the plan's §8 and
`docs/decisions/0002-two-release-relicensing-sequence.md` in the planning repo.
Until then this repository is GPL-2.0-only, for the reason
`src-tauri/Cargo.toml` gives at its `license` key: linking libobs obligates the
whole distributed binary.

`deny.toml` encodes that state mechanically. It has no `deny` list, because
cargo-deny removed that key in 0.18, so every licence not on its allow list
fails, which covers GPL and AGPL and everything else nobody has thought about
yet. Two GPL exceptions stand against it, and deleting them is what proves the
licence exit; it is not a formality:

1. **This crate.** `ninja-recorder`, GPL-2.0-only. WS8 rewrites it.
2. **The capture backend.** One dependency, five crates; see below.

**Do not add a third without asking.** The count is the measurement.

## Copied paths

Every path in the import, grouped by what the plan (§2.1, Appendix D) says
about it.

### Rust: carried forward unchanged (plan §2.1)

- `src-tauri/src/state_machine/`: `mod.rs`, `machine.rs`, `supervisor.rs` and their tests
- `src-tauri/src/recorder/`: `mod.rs` (the trait), `stub.rs`, `libobs/` (incl. `worker_log.rs`, `window.rs`), `devices.rs`, `audio.rs`
- `src-tauri/src/core/`: `mod.rs`, `dispatch.rs` (incl. `every_command_round_trips`; WS2 deletes that test, not the import)
- `src-tauri/src/db/`: `mod.rs` with all 11 migrations, `reconcile.rs`
- `src-tauri/src/lcu/`, `src-tauri/src/live_client/`, `src-tauri/src/dev/`
- `src-tauri/src/retention.rs`, `backfill.rs`, `match_summary.rs`, `trim.rs`, `log.rs`, `update.rs`, `tray.rs`
- `src-tauri/src/lib.rs`, including `ffmpeg_command`
- `src-tauri/src/launch.rs`, `main.rs`
- `src-tauri/src/audio_tracks.rs`, `ddragon.rs`, `fixtures.rs`, `notify.rs`, `probe.rs`: see "Modules Appendix D does not map" below
- `src-tauri/build.rs`, `Cargo.toml`, `Cargo.lock`, `capabilities/`, `icons/`, the three `tauri*.conf.json` files

### Frontend: carried forward so the app still builds (plan §2.1, WS4 strangles it)

- The whole of `src/` (vanilla TypeScript), `index.html`, `dev.html`,
  `tsconfig.json`, `vite.config.ts`, `package.json`, `package-lock.json`

### CI, release and fixtures

- `.github/workflows/ci.yml`, including the ffmpeg staging step (BtbN **lgpl**
  static build; every invocation is `-c copy`) and the libobs staging step
- `scripts/release.mjs`: the version scheme, alpha channel and signed updater config
- `fixtures/`: captured LCU and Live Client Data samples

### Documentation: verbatim, append-only

- `DEVELOPMENT.md` and `docs/*.md` (`architecture.md`, `ci-and-releases.md`,
  `data-model.md`, `dev-portal.md`, `frontend.md`, `product-design.md`,
  `README.md`, `recording-pipeline.md`, `windows-verification.md`)

**Append, never renumber.** Roughly 35 source comments cite `DEVELOPMENT.md`
section numbers. `grep -rn 'DEVELOPMENT.md §' src src-tauri` is the check.

Two of them have been appended to since the import, and neither renumbers
anything: `windows-verification.md` gains **§5.2** (the v2 measurement method's
three empty rows; §5.0 was already "Launch modes", which is why it is not
§5.0.5), and `ci-and-releases.md` gains the new gate list. `DEVELOPMENT.md` is
untouched; v2's §17 has landed with WS2 and §16 with WS1.5, which
leaves §18 as WS8's to write.

### Rewritten for v2, not carried

- `README.md`: rewritten for this repository
- `CLAUDE.md`: rewritten for the v2 layout and the new gate list

## Modules Appendix D does not map

Seven v1 modules have no row in the plan's Appendix D file map. They are copied
as-is and listed here so the omission is recorded rather than silently
inherited. None of them is ambiguous, since each follows the daemon/UI split in
§3.1, but WS3 is where the placement gets made real:

| Module | LOC | What it is | Where §3.1 puts it |
|---|---|---|---|
| `audio_tracks.rs` | 161 | ffmpeg stem extraction from a recorded file | daemon (spawns ffmpeg, writes files) |
| `ddragon.rs` | 988 | Data Dragon champion-asset cache under `app_data_dir()/ddragon` | daemon owns the fetch and the cache; the UI reads the files through the asset protocol |
| `fixtures.rs` | 191 | fixture base-dir plumbing for the dev portal | daemon (`dev_*` is served over the pipe) |
| `notify.rs` | 84 | desktop notifications | daemon; §3.1 names "Desktop notifications: daemon (recording started/ended)" |
| `probe.rs` | 152 | ffprobe/ffmpeg media probing | daemon |
| `recorder/audio.rs` | 423 | `AudioPreset` / `AudioLayout` types used by the `Recorder` trait | daemon, with the trait |
| `recorder/libobs/window.rs` | 54 | window enumeration for `window_capture` | daemon, with the backend |

## Known debt carried in from v1

### ~~WS1.7: `libobs-recorder` is branch-pinned, not tag-pinned~~ (paid, WS1.7)

`src-tauri/Cargo.toml` pinned the capture backend to a **branch**, because at
import the fork carried no tag covering the branch it is built from. It now
pins a tag:

```toml
libobs-recorder = { git = "https://github.com/NinjaGoldfinch/libobs-recorder.git", tag = "v2.0.0", version = "2.0.0" }
```

`v2.0.0` on the fork is the tip of `multi-track-audio` at the moment it was
pinned, `7c651640`, which is the revision `Cargo.lock` already held. The tag
was created against that commit rather than against the branch head, so
nothing about the build changed: the lockfile's revision is byte-identical
either side of the change and only the source URL differs.

**One thing the original entry got wrong**, recorded because #76 will carry it
to the planning repository: the fork did have tags. Five of them
(`libobs_27.2.4` through `libobs_29.1.3`), inherited upstream libobs version
tags pointing at unrelated commits. The true statement was that it had no tag
covering *our* branch, which is a smaller problem than "no tags" and one that
a single `git tag` closed.

`version` is present alongside `tag` for a second reason. A git dependency
without one is a wildcard to cargo-deny, which is why `deny.toml`'s
`[bans] wildcards` sat at `warn`. With both it is `deny`, which is the other
half of what the §8 definition of done asks for.

### WS1.7: the GPL exceptions in `deny.toml`, and why there are five of them

The plan asks for "exactly one named exception for `libobs-recorder`". That is
one *dependency*. cargo-deny names *crates*, and this dependency resolves to
five, all from the same fork and all GPL-2.0 because libobs is:

| Crate | What it is |
|---|---|
| `libobs-recorder` | the crate `Cargo.toml` names |
| `intprocess-recorder` | the in-process half |
| `ipc-link` | the protocol to `extprocess_recorder.exe` |
| `libobs-sys` | the FFI bindings |
| `build-helper` | its build-script support |

They are listed as one block in `deny.toml` and WS8 deletes the block as a
unit, so the property the plan is asking for (the licence exit is a deletion,
not an audit) is preserved exactly. What is not preserved is the number
"one", which is why it is written down here.

The five manifests also spell the licence `GPL-2.0`, a deprecated SPDX
identifier. cargo-deny warns on every run and still matches the exceptions.
It is the fork's manifest, not ours, and WS8 deletes the dependency, so it is
left alone rather than carried as a patch.

### WS1.7: `[bans] wildcards` is `warn`, not `deny`

A git dependency with no `version` key is a wildcard, and cargo-deny has no
per-dependency allow for one. So the key is `warn` until the branch pin above
becomes a tag pin, at which point WS1.7 flips it to `deny`. Leaving it at
`warn` afterwards would let the next wildcard in unnoticed.

### ffmpeg

The bundled ffmpeg is the BtbN **lgpl** static build, staged by CI, invoked
only through `lib.rs::ffmpeg_command` and only ever with `-c copy`. It stays
valid under every capture outcome and survives WS8, because it is a separate
process doing stream copies. It is not a Cargo dependency and so does not
appear in `cargo deny` output; this paragraph is its record.

## Full file list at import

<!-- Generated: git ls-tree -r --name-only 32dcd41 -->

- `.claude/launch.json`
- `.github/workflows/ci.yml`
- `.gitignore`
- `CLAUDE.md`
- `DEVELOPMENT.md`
- `LICENSE`
- `README.md`
- `dev.html`
- `docs/README.md`
- `docs/architecture.md`
- `docs/ci-and-releases.md`
- `docs/data-model.md`
- `docs/dev-portal.md`
- `docs/frontend.md`
- `docs/product-design.md`
- `docs/recording-pipeline.md`
- `docs/windows-verification.md`
- `fixtures/README.md`
- `fixtures/lcu/champion-summary.json`
- `fixtures/lcu/match-history-game.json`
- `fixtures/lcu/ranked-stats.json`
- `fixtures/live-client/captured-allgamedata.json`
- `fixtures/live-client/game-end-lose.json`
- `fixtures/live-client/game-end-win.json`
- `fixtures/live-client/sample-allgamedata.json`
- `fixtures/live-client/summoner-name-mismatch.json`
- `fixtures/make-sample.swift`
- `fixtures/sample.mp4`
- `index.html`
- `package-lock.json`
- `package.json`
- `scripts/release.mjs`
- `src-tauri/.gitignore`
- `src-tauri/Cargo.lock`
- `src-tauri/Cargo.toml`
- `src-tauri/build.rs`
- `src-tauri/capabilities/default.json`
- `src-tauri/capabilities/devtools.json`
- `src-tauri/icons/128x128.png`
- `src-tauri/icons/128x128@2x.png`
- `src-tauri/icons/32x32.png`
- `src-tauri/icons/Square107x107Logo.png`
- `src-tauri/icons/Square142x142Logo.png`
- `src-tauri/icons/Square150x150Logo.png`
- `src-tauri/icons/Square284x284Logo.png`
- `src-tauri/icons/Square30x30Logo.png`
- `src-tauri/icons/Square310x310Logo.png`
- `src-tauri/icons/Square44x44Logo.png`
- `src-tauri/icons/Square71x71Logo.png`
- `src-tauri/icons/Square89x89Logo.png`
- `src-tauri/icons/StoreLogo.png`
- `src-tauri/icons/icon.icns`
- `src-tauri/icons/icon.ico`
- `src-tauri/icons/icon.png`
- `src-tauri/src/audio_tracks.rs`
- `src-tauri/src/backfill.rs`
- `src-tauri/src/core/dispatch.rs`
- `src-tauri/src/core/mod.rs`
- `src-tauri/src/db/mod.rs`
- `src-tauri/src/db/reconcile.rs`
- `src-tauri/src/ddragon.rs`
- `src-tauri/src/dev/events.rs`
- `src-tauri/src/dev/fixtures_api.rs`
- `src-tauri/src/dev/info.rs`
- `src-tauri/src/dev/log_api.rs`
- `src-tauri/src/dev/mod.rs`
- `src-tauri/src/dev/recording_actions.rs`
- `src-tauri/src/dev/recording_api.rs`
- `src-tauri/src/dev/retention_api.rs`
- `src-tauri/src/dev/seed.rs`
- `src-tauri/src/dev/simulate.rs`
- `src-tauri/src/dev/sql.rs`
- `src-tauri/src/dev/trim.rs`
- `src-tauri/src/fixtures.rs`
- `src-tauri/src/launch.rs`
- `src-tauri/src/lcu/champions.rs`
- `src-tauri/src/lcu/client.rs`
- `src-tauri/src/lcu/gameflow.rs`
- `src-tauri/src/lcu/lockfile.rs`
- `src-tauri/src/lcu/match_data.rs`
- `src-tauri/src/lcu/mod.rs`
- `src-tauri/src/lcu/ranked.rs`
- `src-tauri/src/lcu/timeline.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/live_client/client.rs`
- `src-tauri/src/live_client/events.rs`
- `src-tauri/src/live_client/mod.rs`
- `src-tauri/src/live_client/poller.rs`
- `src-tauri/src/live_client/shapes.rs`
- `src-tauri/src/log.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/match_summary.rs`
- `src-tauri/src/notify.rs`
- `src-tauri/src/probe.rs`
- `src-tauri/src/recorder/audio.rs`
- `src-tauri/src/recorder/devices.rs`
- `src-tauri/src/recorder/libobs/mod.rs`
- `src-tauri/src/recorder/libobs/window.rs`
- `src-tauri/src/recorder/libobs/worker_log.rs`
- `src-tauri/src/recorder/mod.rs`
- `src-tauri/src/recorder/stub.rs`
- `src-tauri/src/retention.rs`
- `src-tauri/src/state_machine/machine.rs`
- `src-tauri/src/state_machine/mod.rs`
- `src-tauri/src/state_machine/supervisor.rs`
- `src-tauri/src/tray.rs`
- `src-tauri/src/trim.rs`
- `src-tauri/src/update.rs`
- `src-tauri/tauri.conf.json`
- `src-tauri/tauri.devtools.conf.json`
- `src-tauri/tauri.windows.conf.json`
- `src/bridge.ts`
- `src/desktop.ts`
- `src/dev/ipc.ts`
- `src/dev/main.ts`
- `src/dev/panels/commands.ts`
- `src/dev/panels/database.ts`
- `src/dev/panels/diagnostics.ts`
- `src/dev/panels/fixtures.ts`
- `src/dev/panels/library.ts`
- `src/dev/panels/log.ts`
- `src/dev/panels/overview.ts`
- `src/dev/panels/recorder.ts`
- `src/dev/panels/retention.ts`
- `src/dev/panels/seed.ts`
- `src/dev/panels/simulate.ts`
- `src/dev/registry.ts`
- `src/dev/styles.css`
- `src/dev/types.ts`
- `src/dev/ui.ts`
- `src/devportal.ts`
- `src/dom.ts`
- `src/format.ts`
- `src/icons.ts`
- `src/library.ts`
- `src/main.ts`
- `src/prefs.ts`
- `src/review.ts`
- `src/router.ts`
- `src/settings.ts`
- `src/status.ts`
- `src/styles.css`
- `src/theme.ts`
- `src/toast.ts`
- `src/types.ts`
- `src/update.ts`
- `src/vite-env.d.ts`
- `tsconfig.json`
- `vite.config.ts`
