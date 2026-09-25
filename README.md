# ninja-recorder

A lightweight League of Legends VOD recorder for Windows. It records your games
automatically, tags the timeline with in-game events, and gives you a review
player built for improving rather than editing. **v2 is not a rewrite of that
app; it is the same app with five things changed underneath it.** The stack
that works stays: Tauri, Rust, SQLite, files-as-truth, H.264/AAC in fragmented
MP4, and the `Recorder` trait. What changes is the frontend (vanilla TypeScript
to Svelte 5, by strangler), the IPC contract (two hand-maintained lists to one
generated one), the process model (one Tauri process to a headless daemon plus
a disposable UI), the quality gates (none on the frontend to five in CI), and
the capture backend (embedded libobs to an own WGC/D3D11/Media Foundation
backend): that last one being what eventually lets the licence change.

> **Alpha. The process model has changed, and the loop runs on real hardware.**
>
> This repository started as byte-for-byte v1 at `32dcd41` plus edition 2024, a
> pinned toolchain and the quality gates. It is no longer that. The recorder is
> a headless daemon and the window is a client of it over a named pipe, the
> command and event surface is declared once in Rust and generated into
> TypeScript, the frontend is Svelte 5 throughout with the vanilla shell gone,
> and the capture backend no longer injects into the game.
>
> **The own capture backend is the default.** Recordings are made by Option B
> (WGC, Media Foundation and our own MP4 writer) on Windows 10 2004 and later
> and on Windows 11. An older Windows records on libobs unless something else
> is saved, and libobs stays selectable in Settings → Advanced for one release
> as the fallback.
>
> As of `v2.0.0-alpha.40` the whole loop has run on a live ranked game on
> Windows: the client detected, the game captured, markers placed, the row in
> the library, playback and seeking in the review player, notifications raised
> by a daemon with no window open, and a recording that carried on after the
> window was closed. Capture has run across many live Vanguard-protected games
> since.
>
> **What that does not yet cover** is in
> [docs/windows-verification.md](docs/windows-verification.md), which is
> specific about it: the recorder being killed mid-game, the updater's restart
> after an install that otherwise succeeds, and the resource budgets, where the
> install-size figure is missed rather than met. Expect rough edges and read the
> release notes.
>
> Provenance: [docs/provenance.md](docs/provenance.md) ·
> Plan: [ninja-recorder-v2-plan](https://github.com/NinjaGoldfinch/ninja-recorder-v2-plan)

The design document, the implementation plan, the decision log and the
workstream breakdowns all live in the
**[planning repository](https://github.com/NinjaGoldfinch/ninja-recorder-v2-plan)**,
not here. This repository is the code; that one is the argument for it.

---

## Workstreams

From the implementation plan's §1. Status is as of 2026-09-23; the
[workstream files](https://github.com/NinjaGoldfinch/ninja-recorder-v2-plan/tree/main/docs/workstreams)
in the planning repository carry the detail, including what is done but not yet
verified on Windows.

| WS | What | Gated by | Effort | Status |
|---|---|---|---|---|
| **WS0** | Baseline measurement: install size, idle RAM by Private Bytes | none | 1 wk, part-time | In progress, 2 of 3 |
| **WS1** | Capture backend: P0c go/no-go spike, then Option B; trimmed libobs as fallback | none (spike); gate (build) | 3 wk + 4 wk | In progress; Option B built (WS1.6) and the default since #243 |
| **WS2** | Generated contract: commands *and* events declared once in Rust | none | 2–3 wk | Complete |
| **WS3** | Daemon / UI split over named-pipe JSON-RPC | WS2, WS6 | 3 wk | Code complete, verified 26 of 28 |
| **WS4** | Svelte 5 strangler migration, player last as an imperative island | WS2 | 5–6 wk | Complete and verified |
| **WS5** | Toolchain pin, edition 2024, Biome, Vitest, svelte-check, cargo-deny | none | 2 wk | Complete |
| **WS6** | SQLite WAL, `busy_timeout`, writer + reader pool, `query_only` UI connection | none | 1 wk | Complete |
| **WS7** | Measure against C3 and ship v2.0.0 | everything | 1 wk | Not started |
| **WS8** | Remove libobs, audit, relicense, ship v2.1.0 | one release of WS7 in the field | 1–2 wk | Not started |

Roughly five months of part-time work to v2.0.0, plus a short v2.1.0.

**Partly done, not ticked.** WS5 tasks 5.1–5.5 and 5.7 and WS0 tasks 0.1 and
0.3 have landed; 5.6 waits on WS4.1 and WS0.2 waits on the Windows box. A box
is ticked when its whole workstream is done, so that the table cannot quietly
start meaning "some of it".

**The P0c gate, around week 4, is the only hard fork.** If per-application
audio loopback cannot be made to work, isolated game audio is not achievable
without libobs, and the licence goal has to be weighed against that product
loss before anything downstream continues. Everything after it is sequenced so
that the answer changes *which backend is linked into the daemon* and nothing
else.

## What v2 changes, and what it does not

| | v1 | v2 |
|---|---|---|
| Process model | one Tauri process | `--daemon` owns capture, DB writes, tray, updater; a disposable Tauri UI |
| IPC | `invoke('rpc')` + two hand-written TypeScript lists | one Rust declaration, generated client, CI-checked |
| Frontend | vanilla TS, 12,797 LOC, 0 tests | Svelte 5, tokens, Vitest, `svelte-check` |
| Capture | embedded libobs (GPL, ~200 MB) | WGC → D3D11 → Media Foundation; libobs as fallback for one release |
| SQLite | one `Mutex<Connection>` | WAL, `busy_timeout`, writer + reader pool, `query_only` reader |
| Licence | GPL-2.0 | GPL-2.0 until libobs is gone, then changed at v2.1 |

Unchanged: the state machine and supervisor, the `Recorder` trait, `core`'s
command surface, the LCU and Live Client Data clients, all eleven migrations
and the schema, retention, reconcile, backfill, match summary, trim, the
version scheme, the alpha channel, the signed updater, and ffmpeg (LGPL static,
`-c copy`, separate process). See the plan's §2.1.

## Documentation

| Document | What it covers |
|---|---|
| [docs/provenance.md](docs/provenance.md) | Where this code came from, what is owed because of it, the licence exit |
| [docs/licensing.md](docs/licensing.md) | The WS8 audits: contributors, derived code, ffmpeg, and which releases are GPL |
| [docs/measurement.md](docs/measurement.md) | How install size and memory are measured, so a figure means one thing |
| [CLAUDE.md](CLAUDE.md) | Module ownership, the gates, the rules that are easy to break by accident |
| [docs/architecture.md](docs/architecture.md) | Components, module map, the `Recorder` trait boundary |
| [docs/recording-pipeline.md](docs/recording-pipeline.md) | State machine, events → markers, finalize |
| [docs/data-model.md](docs/data-model.md) | Schema, migrations, reconciliation, retention |
| [docs/frontend.md](docs/frontend.md) | Module ownership, views, IPC surface, theming |
| [docs/dev-portal.md](docs/dev-portal.md) | Driving the backend without League running |
| [docs/ci-and-releases.md](docs/ci-and-releases.md) | CI job graph, the gates, versioning, releases |
| [docs/windows-verification.md](docs/windows-verification.md) | What was checked on real hardware, and what is still open |
| [docs/product-design.md](docs/product-design.md) | The product, its decisions, and how it was actually built |
| [DEVELOPMENT.md](DEVELOPMENT.md) | The *why*: constraints, decisions, alternatives rejected, risks |

`DEVELOPMENT.md` and `docs/*.md` came across from v1 verbatim. **Append to
them; never renumber.** Roughly 35 source comments cite their section numbers.

## Development

```bash
npm install
npm run tauri:dev
```

Prerequisites: Node.js 22 or newer, and the compiler in
[`src-tauri/rust-toolchain.toml`](src-tauri/rust-toolchain.toml): rustup reads
it, so there is nothing to choose.

**`tauri:dev`, not `tauri dev`.** The colon passes `--features devtools`,
which compiles in the dev portal: a second window that seeds the library,
drives the state machine without League running, dry-runs retention and runs
raw SQL. Most of the backend can only be exercised through it. It is a
devtools build, so it keeps its own library under
`%APPDATA%\com.ninjarecorder.app.devtools`, apart from an installed release
([docs/dev-portal.md](docs/dev-portal.md#it-has-its-own-library)).

### The gates, as CI runs them

```bash
npm ci
npx biome ci .                                        # lint + format
npx tsc --noEmit                                      # types
npx vitest run                                        # frontend tests
cd src-tauri && cargo deny check                      # licences + advisories
node scripts/notices.mjs --check                      # THIRD_PARTY_NOTICES.txt is current (needs cargo-about)
cd src-tauri && cargo test
cd src-tauri && cargo test --features devtools
cd src-tauri && cargo clippy --no-deps -- -D warnings
cd src-tauri && cargo clippy --features devtools --no-deps -- -D warnings
```

`npm run format` is the writing half of Biome; `npm run test:watch` is Vitest
in watch mode.

**Note the absent `--all-targets`.** CI does not pass it, so clippy never
compiles the test targets, and a method only the tests call is dead code that
`-D warnings` fails over. Running clippy with `--all-targets` locally compiles
the tests, marks the method used, and hides the failure until CI. See
[CLAUDE.md](CLAUDE.md).

**The Rust project lives at `src-tauri/`, not the repo root.**

### Where work happens

| Layer | Where | Loop |
|---|---|---|
| LCU, Live Client Data, state machine | Dev box, against captured fixtures | seconds |
| VOD library, review UI | Dev box, stub recorder + fixture MP4s | seconds |
| Capture backend | Windows box, `cargo run` | seconds |
| Full integration + Vanguard check | Windows, CI-built installer | occasional |

Installers are produced by CI, never built locally and never cross-compiled.

## Non-negotiable constraints

1. **No injection.** OBS "Game Capture"-style hooking is permanently off the
   table. Vanguard is a kernel anti-cheat and DLL injection is what it exists
   to stop. WGC and display capture only, and that constraint is what the
   whole Option B design is built around, not something bolted onto it.
2. **Official APIs only.** LCU and Live Client Data. No memory reading, no
   packet sniffing.
3. **Lightweight is a feature**, and now a measured one: see
   [docs/measurement.md](docs/measurement.md).

The reasoning is in [DEVELOPMENT.md §1](DEVELOPMENT.md#1-hard-constraints).

## License

**GPL-2.0-only**, inherited rather than chosen: the capture backend embeds
[libobs](https://github.com/obsproject/obs-studio) (GPLv2), which obligates the
whole distributed binary.

Changing that is WS8 and happens at v2.1.0, after libobs is deleted and one
release of v2.0.0 has been in the field, not before.
[`src-tauri/deny.toml`](src-tauri/deny.toml) is what makes the exit mechanical:
GPL is denied with exactly two named exceptions, this crate and the libobs
fork, and deleting them is the proof. See
[docs/provenance.md](docs/provenance.md) and
[docs/licensing.md](docs/licensing.md).

The licences and notices of the Rust crates and npm packages that ship are in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt), which the installer puts
beside the app. It is generated, and CI fails if it is stale
([docs/licensing.md §5](docs/licensing.md#5-third-party-notices-85)).

**The change is not retroactive.** Every release before `v2.1.0`, here and in
[`ninja-recorder-deprecated`](https://github.com/NinjaGoldfinch/ninja-recorder-deprecated/releases),
is GPL-2.0-only, stays GPL-2.0-only, and stays available.
