<!--
Delete any section that does not apply. An empty heading is worse than no
heading — it reads as something forgotten rather than something considered.
-->

## What this changes

<!-- And, if it is not obvious, what it deliberately does not. -->

Closes #

## Why

<!-- The reasoning, not the diff. The diff is right there. -->

## Verification

<!-- What you actually ran, and on what. "CI is green" is the weakest form of
     this; "reproduced the failure locally first, then confirmed the fix" is the
     strongest. Say which platform — a Linux dev box does not compile
     `recorder/libobs/`, `recorder/devices.rs`, or anything else behind
     `cfg(target_os = "windows")`, and clippy's silence about code it never
     compiled is not evidence. -->

## Checklist

- [ ] The nine gates pass locally, or I have said which did not and why
      (`biome ci .` · `tsc --noEmit` · `vitest run` · `cargo deny check` ·
      `cargo test` ×2 · `cargo clippy -D warnings` ×2)
- [ ] **The docs that describe this behaviour are updated in this PR.**
      `CLAUDE.md` has the table of which document goes with which change — a
      diagram that lies is worse than no diagram
- [ ] `DEVELOPMENT.md` and `docs/*.md` are **appended to, never renumbered** —
      ~35 source comments cite their section numbers
      (`grep -rn 'DEVELOPMENT.md §' src src-tauri`)
- [ ] Migrations are appended, not edited — shipped builds have run the old ones
- [ ] No `{@html}` on any recording-derived string, and no new `escapeHtml`
      bypass — `reconcile` imports whatever the user drops in the folder
- [ ] Every ffmpeg spawn still goes through `lib.rs::ffmpeg_command`
- [ ] No new `deny.toml` licence exception (a third one needs a conversation,
      not a commit)
- [ ] If this touches Windows-only code, I have said how it was checked —
      because nothing on a Linux or macOS dev box compiles it
