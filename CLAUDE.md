# CLAUDE.md

Guidance for Claude Code and anyone else working in this repo.

## The project

`ninja-recorder` — a League of Legends VOD recorder. One Tauri v2 process:
Rust core in `src-tauri/`, vanilla-TypeScript frontend in `src/`.

Read [docs/architecture.md](docs/architecture.md) before making a change you
can't fully see the blast radius of.

---

## Documentation is part of the change

**A behaviour change and the doc update that reflects it belong in the same
commit.** A diagram that lies is worse than no diagram. Use this table to work
out what to touch.

| If you change… | Update… |
|---|---|
| `src-tauri/src/state_machine/` | [docs/recording-pipeline.md](docs/recording-pipeline.md) — the state diagram, the transition table, the edge-case table |
| `src-tauri/src/lcu/`, `src-tauri/src/live_client/` | [docs/recording-pipeline.md](docs/recording-pipeline.md) — the sequence diagram, signal cadences, the events→markers flowchart |
| `src-tauri/src/recorder/` | [docs/architecture.md](docs/architecture.md) — the trait-boundary diagram; and DEVELOPMENT.md §2 if the *decision* changed |
| A `db/mod.rs` migration, or any schema change | [docs/data-model.md](docs/data-model.md) — the ER diagram **and** the migration-history table |
| `src-tauri/src/db/reconcile.rs` | [docs/data-model.md](docs/data-model.md) — the reconciliation flowchart |
| `src-tauri/src/retention.rs` | [docs/data-model.md](docs/data-model.md) — the retention flowchart, and when enforcement runs |
| The Tauri command list in `lib.rs` | [docs/frontend.md](docs/frontend.md) — the command table; and `src/dev/registry.ts` (see "drift" below) |
| Anything in `src/` | [docs/frontend.md](docs/frontend.md) — the module graph, view diagram, or theming flow |
| Anything in `src/dev/` or `src-tauri/src/dev/` | [docs/dev-portal.md](docs/dev-portal.md) — the panel table |
| `.github/workflows/ci.yml` | [docs/ci-and-releases.md](docs/ci-and-releases.md) — the job graph and build flowchart |
| Capture-backend status, or anything verified on real Windows hardware | [docs/windows-verification.md](docs/windows-verification.md), the README status blockquote, DEVELOPMENT.md §2.2 / §3.4, and the caveat block in `ci.yml`'s release notes |
| A *decision*, constraint, or trade-off | [DEVELOPMENT.md](DEVELOPMENT.md) — the "why" doc |
| Scope: a feature shipped, dropped, or reordered | [docs/product-design.md](docs/product-design.md) — the phase table and implementation history |

### Which document gets the change

- **DEVELOPMENT.md** = *why*. Constraints, decisions, alternatives rejected,
  risks. If someone might later ask "why is it like this?", the answer goes
  here.
- **docs/\*.md** = *what and how*. Diagrams, module maps, runtime flows,
  schemas. If someone is trying to find or follow the code, it goes here.
- **docs/product-design.md** = *the product and its build history*. What the
  product is for, the decisions that shaped it, what each phase delivered, and
  what the plan got wrong. Retrospective, not a spec.
- **README.md** = *what it is and how to run it*. Keep it short; link out.

Do not duplicate prose between them — link instead. Duplicated prose is
duplicated maintenance, and one copy always goes stale first.

### DEVELOPMENT.md section numbers are load-bearing

Roughly 35 source comments cite `DEVELOPMENT.md §2.2`, `§3.4`, and so on. Add
sections and rewrite their contents freely, but **do not renumber existing
ones** without updating every citation:

```bash
grep -rn 'DEVELOPMENT.md §' src src-tauri
```

### Do not reintroduce phase numbers

The build ran as eleven numbered phases and those numbers used to appear in
source comments (`Phase 6`, `pending Phase 8`). They have all been rewritten to
say what they mean, and the numbering now lives in one place:
[docs/product-design.md](docs/product-design.md), which carries a mapping table
for reading old commits and issues. New comments should describe behaviour and
link to the document that explains it.

### Diagrams

Mermaid in fenced ```` ```mermaid ```` blocks. GitHub renders it natively, it
diffs as text in review, and it needs no tooling. No image files, no external
diagram editors.

---

## Working in this repo

### Running the app

```bash
npm install
npm run tauri:dev    # NOT `tauri dev` — the colon passes --features devtools
```

Without `--features devtools` the dev portal window and every `dev_*` command
are absent, and most of the backend becomes undrivable without a real League
client. See [docs/dev-portal.md](docs/dev-portal.md).

### The Rust project is at `src-tauri/`, not the repo root

```bash
cd src-tauri && cargo test
cd src-tauri && cargo test --features devtools
cd src-tauri && cargo clippy --all-targets -- -D warnings
```

CI runs the Rust tests and clippy **both with and without** `--features
devtools`, and clippy with `-D warnings`. A change that only compiles one way
fails.

### Frontend checks

```bash
npx tsc --noEmit
```

## Conventions that are easy to violate by accident

- **Never inject into the game process.** WGC/display capture only. This is a
  hard constraint, not a preference — see DEVELOPMENT.md §1.1.
- **Pure decision + thin I/O wrapper.** `state_machine::machine`,
  `db::reconcile` and `retention::select_for_deletion` are pure and directly
  unit-tested; their wrappers are deliberately too small to hide a bug. Adding
  I/O or a clock read to a pure function removes its test coverage.
- **Append migrations, never edit them.** Shipped builds have already run the
  old ones.
- **`escapeAttr` for attribute values, `escapeHtml` for text nodes.**
  `reconcile` imports any video file the user drops in the folder, so
  displayed recording names are not trusted input.
- **Two command lists in `lib.rs`.** `generate_handler!` can't host a
  `#[cfg]`, so the production list is spelled out twice. Adding a command
  means editing both, plus `src/dev/registry.ts` — the Commands panel's drift
  banner catches a mismatch, but only once someone opens it.
- **Don't remove `theme.ts`'s matchMedia `change` listener.** It is the only
  thing making the "System" theme follow the OS, and no test covers it.
- **Don't attach the devtools build to a release.** It carries raw SQL,
  arbitrary row writes, a DB wipe and state-machine injection.

## Git

Commit messages follow `type(scope): imperative summary` — e.g.
`fix(lcu): show the Riot ID as the summoner name`. No AI attribution in
commits, PRs, or release notes.
