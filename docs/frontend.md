# Frontend

Svelte 5, under `src/lib/`. Since WS4.6 the app renders **every** element it
has: `index.html`'s body is one `<div id="app-root">`, and what remains in its
head cannot move into a component, being the anti-flash boot script and the
stylesheet that script depends on.

The player is the exception to everything below and stays an **imperative
island** on purpose: a `<video>`'s `currentTime` is not state anything should
be diffing.

The organising principle is the same on both sides: **state ownership, not
widgets**. Each module owns exactly one piece of mutable state and is the only
place that writes it. A store is that rule with runes rather than module
variables.

---

## Module graph

```mermaid
flowchart TB
    MAIN["main.ts<br/><small>composition root: owns nothing</small>"]
    ROUTER["router.ts<br/><small>owns: which view is showing</small>"]
    THEME["theme.ts<br/><small>owns: html[data-theme]</small>"]
    PREFS["prefs.ts<br/><small>owns: the preference cache</small>"]
    DESK["desktop.ts<br/><small>owns: the browser behaviours we suppress</small>"]
    APP["lib/App.svelte<br/><small>the root: app bar, strip, five views,<br/>quit dialog, toast</small>"]
    SHELL["lib/components/shell/<br/><small>AppBar, DaemonStrip, CaptureStrip,<br/>QuitDialog, Toast</small>"]
    STATUS["lib/stores/status.svelte.ts<br/><small>owns: the poll timer</small>"]
    DAEMONS["lib/stores/daemon.svelte.ts<br/><small>owns: whether the recorder is there</small>"]
    CAPS["lib/stores/capture.svelte.ts<br/><small>owns: what the last recording lost</small>"]
    QUITS["lib/stores/quit.svelte.ts<br/><small>owns: the two-process quit flow</small>"]
    TOASTS["lib/stores/toast.svelte.ts<br/><small>owns: the transient message</small>"]
    ABOUT["lib/stores/about.svelte.ts<br/><small>owns: the three live About lines</small>"]
    LIBV["lib/components/library/<br/><small>Library, Row, Toolbar, StatsBar,<br/>Loadout, Matchup, Slot, RowActions</small>"]
    LIBS["lib/stores/library.svelte.ts<br/><small>owns: the row set + every control</small>"]
    ICONS["lib/stores/icons.svelte.ts<br/><small>owns: when art has arrived</small>"]
    REVV["lib/components/review/<br/><small>Review (imperative island), Timeline,<br/>PlayerControls, MarkerList, MarkerTimes</small>"]
    REVS["lib/stores/review.svelte.ts<br/><small>owns: which recording, its markers,<br/>samples and window</small>"]
    SETV["lib/components/settings/<br/><small>Settings, Appearance, BackgroundTray,<br/>Notifications, AudioSettings, Storage,<br/>Advanced, About, Update, UpdateNotes, SettingRow</small>"]
    SETS["lib/stores/settings.svelte.ts<br/><small>owns: autostart, audio, capture backend,<br/>retention, the folder, mirrored prefs</small>"]
    UPD["lib/stores/update.svelte.ts<br/><small>owns: the update status</small>"]
    TL["lib/timeline/<br/><small>window, clusters, graph, stem,<br/>markers, navigate · pure, tested</small>"]
    LIBP["lib/library/<br/><small>filters, sort, stats, scoreboard,<br/>problems · pure, tested</small>"]
    SETP["lib/settings/<br/><small>notes, backfill, retention, audio,<br/>capture, update, about · pure, tested</small>"]
    REVP["lib/review/<br/><small>hotkeys, playback<br/>pure, tested</small>"]
    RFV["lib/components/reviewform/<br/><small>ReviewForm, RatingControl</small>"]
    RFS["lib/stores/gameReview.svelte.ts<br/><small>owns: the open game, its draft review,<br/>the autosave queue</small>"]
    OBV["lib/components/objectives/<br/><small>Objectives</small>"]
    OBS["lib/stores/objectives.svelte.ts<br/><small>owns: the objectives list and its tab</small>"]
    RFP["lib/reviewform/<br/><small>fields, ratings, autosave, sheet<br/>pure, tested</small>"]
    BRIDGE["bridge.ts<br/><small>composition root: picks a transport,<br/>exposes the generated client</small>"]
    TRANSPORT["lib/transport/<br/><small>pipe.ts (live) · mock.ts<br/>invoke.ts kept for the dev portal</small>"]
    CONTRACT["lib/contract/<br/><small>GENERATED from Rust</small>"]
    FMT["format.ts<br/><small>pure formatters + label fallbacks</small>"]
    TYPES["types.ts<br/><small>mirrors the Rust serde structs</small>"]
    STYLES["lib/styles/<br/><small>tokens.css · app.css · dev.css</small>"]

    MAIN --> ROUTER
    MAIN --> APP
    ROUTER -.->|"mountApp / unmountApp"| APP
    MAIN --> THEME
    MAIN --> PREFS
    MAIN --> STATUS
    MAIN --> DAEMONS
    MAIN --> CAPS
    MAIN --> QUITS
    MAIN --> DESK
    DESK --> BRIDGE
    BRIDGE --> TRANSPORT
    BRIDGE --> CONTRACT
    BRIDGE --> TYPES
    STATUS --> LIBS
    STATUS --> UPD
    STATUS --> ABOUT
    STATUS --> BRIDGE
    APP --> SHELL
    APP --> LIBV
    APP --> REVV
    APP --> SETV
    APP --> RFV
    APP --> OBV
    LIBV --> RFS
    RFV --> RFS
    RFV --> RFP
    RFV --> FMT
    RFS --> RFP
    RFS --> BRIDGE
    RFS --> TOASTS
    OBV --> OBS
    OBS --> BRIDGE
    OBS --> RFP
    OBS --> TOASTS
    SHELL --> DAEMONS
    SHELL --> CAPS
    CAPS --> TRANSPORT
    CAPS --> LIBP
    REVV --> LIBP
    SHELL --> QUITS
    SHELL --> TOASTS
    SHELL --> UPD
    SHELL --> ABOUT
    LIBV --> LIBS
    LIBV --> LIBP
    LIBV --> ICONS
    LIBV --> REVS
    LIBV --> FMT
    LIBS --> LIBP
    LIBS --> BRIDGE
    REVV --> REVS
    REVV --> REVP
    REVV --> TL
    REVV --> FMT
    REVS --> BRIDGE
    SETV --> SETS
    SETV --> SETP
    SETV --> UPD
    SETV --> ABOUT
    SETV --> LIBS
    SETV --> THEME
    SETS --> PREFS
    SETS --> BRIDGE
    PREFS --> BRIDGE
    UPD --> BRIDGE
    UPD --> TOASTS
    FMT -.->|"type-only"| TYPES
    style MAIN fill:#ede7f6,stroke:#5e35b1
    style BRIDGE fill:#e3f2fd,stroke:#1565c0
    style APP fill:#fff3e0,stroke:#ef6c00
    style SHELL fill:#fff3e0,stroke:#ef6c00
    style DAEMONS fill:#fff3e0,stroke:#ef6c00
    style CAPS fill:#fff3e0,stroke:#ef6c00
    style QUITS fill:#fff3e0,stroke:#ef6c00
    style TOASTS fill:#fff3e0,stroke:#ef6c00
    style ABOUT fill:#fff3e0,stroke:#ef6c00
    style REVV fill:#fff3e0,stroke:#ef6c00
    style REVS fill:#fff3e0,stroke:#ef6c00
    style SETV fill:#fff3e0,stroke:#ef6c00
    style SETS fill:#fff3e0,stroke:#ef6c00
    style UPD fill:#fff3e0,stroke:#ef6c00
    style LIBV fill:#fff3e0,stroke:#ef6c00
    style LIBS fill:#fff3e0,stroke:#ef6c00
    style ICONS fill:#fff3e0,stroke:#ef6c00
    style STYLES fill:#fff3e0,stroke:#ef6c00
    style TL fill:#e8f5e9,stroke:#2e7d32
    style LIBP fill:#e8f5e9,stroke:#2e7d32
    style SETP fill:#e8f5e9,stroke:#2e7d32
    style REVP fill:#e8f5e9,stroke:#2e7d32
    style RFV fill:#fff3e0,stroke:#ef6c00
    style RFS fill:#fff3e0,stroke:#ef6c00
    style OBV fill:#fff3e0,stroke:#ef6c00
    style OBS fill:#fff3e0,stroke:#ef6c00
    style RFP fill:#e8f5e9,stroke:#2e7d32
```

Orange is a component or the store that feeds it, green is pure and directly
tested, and the two roots are the only modules that compose rather than own.

**Four modules sit outside the tree and are meant to.** `main.ts` composes,
`theme.ts` owns `html[data-theme]`, `desktop.ts` suppresses browser behaviours
at the document level, and `router.ts` decides which view is showing. None is
a view and none looks up an element. Beside them sit the shared libraries the
components import: `bridge`, `format`, `prefs`, `icons`, `router` and `types`.
They are modules rather than markup and were never what "vanilla" meant here.

`types.ts` sits apart deliberately: putting each shape beside its first
consumer would make `bridge` -> `review` -> `bridge` a cycle.

**The dev portal is a second tree, not a branch of this one.** `dev.html`
mounts `lib/components/dev/DevApp.svelte`, which renders eleven panels against
`lib/dev/` and `src/dev/`; nothing in the app proper may import any of it,
because all of it is compiled out of a shipped build. It has its own document,
its own stylesheet and its own entry point, and
[docs/dev-portal.md](dev-portal.md) is its map.

### How a command reaches the backend

Since WS2.6 `bridge.ts` is a composition root rather than an implementation. It
picks a transport and exposes the generated client over it.

```
view  ->  client (generated)  ->  Transport  ->  invoke("rpc_call", …)  ->  daemon
                                            ->  fixtures                    in the vite dev server
```

**The transport is an interface with three implementations, and the live one
changed under every caller without one of them noticing.** `pipe.ts` is what a
Tauri session uses since WS3.4: commands go to `rpc_call`, which forwards them
over the pipe to the daemon, and the short direct-command list stays here for
the ones that drive the desktop shell. `mock.ts` is in-memory. `invoke.ts` is
the old Tauri transport, which ran commands in the process hosting the webview;
nothing selects it any more, and it is kept because it is still the correct
transport for a process that owns its own `Ctx`.

That swap is the whole argument for having put an interface at this seam before
the daemon needed one. The views, the generated client and `call` are unchanged
by it.

**`daemon.ts` is the one thing that renders the connection itself.** A strip
under the app bar, shown when the daemon is not there and hidden when it is,
because that is a *state* rather than an event: a toast that vanished while the
recorder was still missing would be worse than saying nothing. Version skew gets
its own message and no button, since a UI and a daemon from different builds
cannot be made to agree by waiting, and the one thing the window must not do is
tell the daemon to quit while it may be recording.

**A capture failure gets a strip of its own** (#10, DEVELOPMENT.md §2.6).
`stores/capture.svelte.ts` subscribes to the `captureProblems` event and
`shell/CaptureStrip.svelte` shows it under the recorder strip, in the same
shape: "The last recording was saved without game audio (the failing call and
its HRESULT). If this keeps happening, please report it with your Windows
version (Windows build 19045)", or, for a game that was not recorded at all,
the same as an error. Unlike the recorder strip it is an event rather than a
state, so it does not clear itself: it stays until it is dismissed, and the
next problem replaces it. The recording keeps the same list in its
`diagnostics_json`, and `library/problems.ts` turns that into the library row's
"Recorded without game audio" (in the slack column, with every reason in its
tooltip) and the review page's full line under the heading, so a window that was
closed at the time still says it. Every reason is untrusted text and is only
ever interpolated; `CaptureStrip.test.ts`, `Row.test.ts` and `Review.test.ts`
each feed one markup and check it comes out as text.

**It also decides when the views may fetch.** `whenDaemonReachable` runs its
caller once the daemon is reachable and again on every reconnect, and `main.ts`
puts the whole startup batch behind it: the library, the disk usage and the
preferences. They used to run on load, which is a race the window loses.
`DaemonLink::connect` returns immediately and keeps connecting in the
background, so the first paint happens before the handshake, and on a first
launch after an install the window is starting the daemon itself. Every one of
those fetches came back with "not connected to the recorder" and reported it as
a failure, so a healthy install opened with a red error box that cleared itself
a moment later.

Not making the call is the fix rather than swallowing its error, which keeps a
genuine failure on a live connection loud. The reconnect half is the part that
never worked at all: the strip cleared itself when the daemon came back and
nothing re-read anything, so a window that lost its recorder went on showing
whatever it held at the moment the connection died.

**`pipe.ts` also carries the other direction**, which `invoke.ts` never had to.
`subscribe` listens on three channels: `snapshot` replaces the frontend's world
on every handshake, `event` updates it, and `daemon-health` says whether there
is a connection at all. A snapshot is not an event and they are deliberately not
one channel: folding a fresh world in as though it were an update would merge it
into a stale one.

**The mock is a transport now, not a branch inside `call`.** It used to be two
thirds of `bridge.ts`, reachable only by being outside Tauri with
`import.meta.env.DEV` set. As a transport a test picks it explicitly, which is
what lets Vitest drive the *real* generated client with neither a daemon nor a
WebView2 behind it. It still tree-shakes out of a production build, because
`import.meta.env.DEV` is statically false there.

**`call` survives, deliberately.** The nine view modules are written against
it, and WS2.6's exit criterion is that frontend behaviour does not move, so
rewriting all of them was not this task. New code should prefer `client`, whose
method names, arguments and return types come from the Rust declaration; `call`
is what WS4 strangles as each view is rewritten.

### Suppressed browser behaviour

`desktop.ts` is the only module that exists to make things *not* happen. A
webview arrives as a page, with selectable text, a browser context menu, drag
images and F5, and a window wants none of it. What each half handles:

| Behaviour | Where | Exemption |
|---|---|---|
| Text selection | `styles.css`, `user-select` | Form fields, `code`, `.mono`, `.about-list dd`, and anything marked `.selectable` |
| Context menu | `contextmenu` | Text fields (it is the Cut/Copy/Paste menu there); every build that can inspect |
| Drag images | `dragstart` | Text fields |
| Middle-click autoscroll | `mousedown`, button 1 | none |
| Reload (F5, Ctrl+R) | `keydown` | Every build that can inspect |
| Print (Ctrl+P) | `keydown` | none |

"Every build that can inspect" means the vite dev server (`import.meta.env.DEV`)
or a `devtools` build, detected through `bridge.ts`'s `hasDevCommands` probe.
The flag is read when the event fires, not captured at init, so the few
milliseconds before that probe resolves simply behave like a shipped build.

The dev portal (`dev.html`) does none of this. It is a debugging
surface: log output and query results are there to be selected and copied, and
"Inspect element" is a feature of the window.

The reasoning behind all of it is in
[DEVELOPMENT.md §5.1](../DEVELOPMENT.md).

### The library is a list, not a grid

One row per game. A library is scanned rather than browsed. The question is
almost always "which game was that", answered by champion, result and roughly
when, and a card grid answers that in two dimensions when one would do. Rows
also left somewhere for the scoreboard, items and team compositions to go
without a second redesign (#85), which is where all three now are.

**A stacked block on the left sets the row's height**: what the game was, when
it was, which patch, how long it ran, and how it went. Four short lines rather
than four columns, because none of them is a number worth comparing down the
list. Together they answer "is this the game I mean", which is read once per
row and then never again.

What *is* worth comparing gets a column: the champion, and the KDA. Deaths are
coloured and the slashes are not, so the eye lands on the middle number without
having to read the other two.

Every column is *capped*, so values line up down the list and a column can be
read vertically without the eye re-finding it on each row. Cells ellipsize
rather than widening the row.

**The leftover width collects in one place, before the actions.** A single
column at `1fr` stretched the champion cell across half the window and threw
everything else at the right edge, so the row read as two unrelated clusters
with a hole between them. With every data column capped they stay one group at
the left, the actions stay pinned right, and the slack sits between them.

The team compositions were once earmarked for that slack and took a column of
their own instead. A fixed grid of squares dropped into a `1fr` track leaves
the leftover width *inside* a data column, where it is invisible to read but
real to every column added after it; slack that stays slack keeps the rule
above true rather than nearly true.

**One thing may sit in the slack: what a capture failure cost** (#10). A
recording whose `diagnostics_json` lists capture problems says "Recorded
without game audio" there, right-aligned, ellipsized, with every reason in its
tooltip. It is the one cell that is empty on almost every row, so a row that
says it is still the same shape as one that does not.

**The lane matchup, not the lobby.** The row shows the one opponent who
played your position, with their champion, their line and their build, where it
used to show all ten portraits. Ten champions told you who was in the game;
one tells you who you actually played against, which is what a person is
reconstructing when they scan a library. "The Darius game" is a matchup, not a
lobby. The other nine are still in `scoreboard_json` for anything that wants
them.

**The opponent is looked up, never guessed.** Every player on the board carries
a `position`, so the enemy in our lane is a filter rather than an index into a
list whose order nothing promises. Where the position is missing, which covers
every recording made before the field existed and any mode with no positions to
assign, there is no matchup, and the block **says so in words**. Seven blank
boxes beside a "vs" read as art that failed to load, which is a bug report
waiting to happen; "No matchup recorded" reads as what it is. The block keeps
its width either way, so the columns on both sides stay put.

**The position has to survive the LCU rebuild.** The deferred patch replaces the
live scoreboard with the LCU's (#127), which is better at almost everything:
champion ids rather than display names, settled numbers rather than the last
poll. It is *worse* at position: that comes from Riot's `lane`/`role` inference
and is sometimes absent. Replacing the board wholesale therefore threw away the
positions the live capture had, and every patched recording lost its matchup.
`prefer_live_positions` keeps them, matched on team **and** champion together
since neither is unique alone; a blind-pick game can have the same champion on
both sides.

**The live value wins where both know**, which is the rule `role` has followed
all along: the inference is a fallback for a game the poller missed, never a
correction. Filling only the gaps left the quieter half of the bug in place:
an inference that confuses two lanes produces a *present but wrong* position,
the matchup then picks the enemy in the wrong lane, and the row shows a
plausible opponent who is not the one you played. An empty matchup announces
itself; a wrong one does not.

**The art tracks are stated, not `auto`.** Every `.vod-row` is its own grid, so
an `auto` track sizes to *that row's* content, and a row whose player sold an
item, or whose scoreboard is missing, computed different widths from the row
above it. The blocks are fixed grids of fixed squares internally, so the widths
were never really variable; they only looked it. Stating them is what makes a
column read down the list, which is the entire reason the row is a grid at all.

**It sheds in two stages**, because its halves are worth different amounts. The
opponent's build is the wide part, seven squares and their gaps at 166px, and
the least of what the block says; who you played and how they did survives
another 240px of narrowing. Below about 1440px the build goes, below about
1200px the rest follows. The thresholds are the row's own tracks rather than
round numbers: with the build it needs about 1430px, without it about 1180px.

### What a row says when the data is missing

Match metadata arrives from two independent sources, Live Client Data
during the game and the LCU after it (see
[recording-pipeline.md](recording-pipeline.md) §4), so a row can carry
either, both or neither. `format.ts` owns the fallback chains rather than
scattering `??` through the row template:

| Slot | Chain | Why it stops there |
|---|---|---|
| Title (`vodTitle`) | `champion` → game mode → filename | Never empty. The filename is untrusted input, so the caller still escapes it |
| Heading (`vodHeading`) | `vodTitle` + ` vs <opponent>` + the outcome word | The long form, for the review view's heading and the row's accessible name, where there is room for what actually identifies a game. Each half is added only when known, so it degrades through `Viego vs Darius`, `Viego` plus the outcome, and `Viego` alone, rather than emitting `vs undefined`. An undecided game says nothing about a result, exactly as the row's own outcome word does |
| Queue (`queueOrModeLabel`) | `queue` id → `game_mode` | `CLASSIC` renders as "Summoner's Rift", the *map*: the mode string cannot tell blind from draft from ranked, and naming one would be a guess in a slot read as fact |
| Patch (`patchLabel`) | stored build → its first two parts, a major from 16 on shown ten higher | The column keeps the whole build string the game reported, because the build is what tells two sides of a hotfix apart. Riot numbers patches by year from 2026 while the build kept its old sequence, so build 16.19 is patch 26.19 (#280); majors below 16 predate that and show as they are. The label is what the patch filter lists and sorts, and the mapping keeps it numerically ordered |
| KDA (`formatKda`) | all three or nothing | A partial KDA reads as a real one. The ratio (`kdaRatio`) is a hover hint, not a fourth number in a column three numbers wide |
| Role | Live Client Data's position → the LCU's inference → `Unknown` | The live value is what the game assigned; the LCU's `timeline.lane`/`role` is Riot working it out afterwards and confuses top with jungle, so it fills a gap rather than correcting one. `Unknown` is written out rather than left blank, because a row that hides an empty slot is a different shape per recording |
| Outcome | the leading accent, plus the word on the left block's last line | Undecided rows are excluded from the win-rate tile too, so an unknown never reads as a loss; it gets the neutral edge, no wash and no word. A Win/Loss badge used to sit in its own column and was dropped as redundant with the edge; the word moved into the sub-line rather than being dropped with it, because the accent alone is colour only |
| Rank (`rankLabel`, `lpLabel`) | `tier` + `division` → the missing-value placeholder | The ladder a game was played at. That placeholder covers three different things the column cannot tell apart (a queue with no ladder, a player unranked in it, and a patch that landed too late for the reading to still describe the game) so the row does not pretend to. LP is shown only beside a tier, since a number with no scale is not a standing. Master and above have no division and are labelled with none |
| When (`formatRelative`) | relative inside a week → absolute date | "6 weeks ago" is worse than a date at that distance: nobody counts weeks, and the date is what a person searches their memory by. The absolute form is on the `title` either way |

An unrecognised queue id shows as `Queue 1234` and an unrecognised mode
shows as itself. Both are honest; neither invents a name.

### Which blanks the backfill can fill, and which are blank forever

A row made before the metadata pipeline shipped, or imported by `reconcile`
from a folder the user pointed at, starts with almost every slot missing. The
backfill (Settings → "Fill in missing match data"; mechanics in
[data-model.md](data-model.md)) fills some of that from the client's match
history. **It cannot fill all of it, and the difference is not arbitrary:** it
is exactly the line between what the game *reported afterwards* and what only
something watching *during* the game could have seen.

| Blank on the row | The backfill | Why |
|---|---|---|
| Champion, result, KDA | fills | Straight off the match-history document |
| Queue, role, patch | fills | Same document. `role` is Riot's own `lane`/`role` inference, not the live position; see the table above |
| CS | fills, with the scoreboard | Written only when there was no scoreboard at all |
| Items, spells, runes, the ten champions | fills | The scoreboard is rebuilt from the same document, so it arrives as champion *ids* rather than display names |
| The gold curve | fills **only if the recording already has samples** | The curve has to be placed in the video, and the offset for that is read off an existing sample (`sample_alignment_offset`). A recording that never had a live poller has no offset, and a guessed one would draw the right curve at the wrong times |
| Kill diff, CS diff curves | **never** | The advantage curve's other two metrics are the live poller's own arithmetic. Match history has no per-second series but gold |
| The marker timeline | **never** | Live Client Data is gone the moment the game ends, and it was the only thing that saw the events. See [DEVELOPMENT.md §3.2](../DEVELOPMENT.md) |

The last two are the ones worth knowing before running it. A backfilled
recording gets a row that reads completely and a review view that is still
half empty: the gold curve draws, the other two metrics say they have no
data, and the timeline carries no glyphs at all. That is not a bug in the
backfill; those recordings never held the events, and nothing can put them
back.

Two properties inherited from the mechanism, because they show up as
surprises otherwise. **It only ever fills**, so a value already on the row
survives a run. It matches recordings to games on the clock, and filling a
gap on a heuristic is fair where overwriting good data on one is not. And it
**refuses outright when more than one game overlaps** a recording, so a row in
a back-to-back session can come back still blank; that is the refusal working,
not a miss.

### The filter bar

Five filters and a sort, all of them (with the stats bar above them) operating
client-side over the already-fetched row set. That is fine at solo-user
library sizes and would need real pagination if that stops being true.
Champion (a search box), result and pinned-only were there first; queue, role
and patch complete the set #85 called for, and every one of them reads a
column that already exists.

| Filter | Reads | Built from |
|---|---|---|
| Champion | `vodTitle(row)` | free text |
| Queue | `queueOrModeLabel(row)` | the rows in the library |
| Role | `role` | the rows in the library |
| Result | `win` | fixed: all / wins / losses |
| Patch | `patchLabel(row.patch)` | the rows in the library |
| Pinned only | `pinned` | a checkbox |

**The three derived lists come from the data, not from a vocabulary.** Patch
is open-ended and could not be enumerated ahead of time at all. Queue ids are
a table `format.ts` only partly names, and `Queue 1234` is a real label a
hard-coded list would have no entry for. And a fixed list offers "Ranked Flex"
to somebody who has never queued it, which is a control that can only ever
empty the list. A facet with fewer than two things to choose between is
`disabled` rather than hidden, so the bar keeps one shape as a library grows,
unless it is the facet currently filtering, which is never disabled:
retention or a delete can take the library down to the one value already
selected, and greying the control there strands a selection with no way to
undo it.

They are derived from the **whole** library, not from what the other filters
leave. Facets that narrow as you use their neighbours are how a person ends up
holding a selection they can no longer see the way out of.

**Queue filters on the label, not the id**, because the label is what the row
shows, and it is the merged `queue`-then-`game_mode` chain, so a row with no
queue id still files under what it says. Two ids that print the same name
(1700 and 1710 are both "Arena") group together, which is the intent.

**"Unknown" is a value you can filter *to*,** offered only when something is
actually missing it. "Which of my games never got a role" is the question the
`Unknown` on the row itself prompts, and the backfill leaves plenty of them.
See [data-model.md](data-model.md) for what it can and cannot fill.

**An empty result says which kind of empty it is.** "Nothing recorded yet" and
"everything is filtered out" are different problems with different next steps,
and the first message used to be the only one there was, which read as data
loss the moment a filter matched nothing. The filtered case names the total it
is hiding and carries the Clear filters button, which resets the five filters
and deliberately leaves the sort alone: sort hides nothing, and resetting it
would throw away an order the user chose.

## Views

Five top-level sections in one document, toggled by `router.ts`. Before it
existed, each view flipped its own and its sibling's `hidden` attribute from
two files that knew nothing about each other.

```mermaid
stateDiagram-v2
    [*] --> library
    library --> review: click a VOD card
    review --> library: back
    library --> game: 📝 on a VOD card
    game --> library: back (saves first)
    library --> objectives: objectives button
    objectives --> library: back
    library --> settings: settings button
    review --> settings: settings button
    settings --> library: close (always returns to library)
```

### The review form (WS9)

`game` is the review form for one game and `objectives` the list it reviews
against. **The form is its own view in P0, not a rail beside the player.** P0
builds nothing video-related, and a separate view keeps the form clear of the
player's document-level hotkeys, which only answer while `review` is showing.
P1 moves it next to the player.

- **The whole review is saved at once.** The form edits a draft, and
  `reviewform/autosave.ts` writes all of it 600 ms after the last change. It
  never runs two saves at once, and the last value typed is always the last
  value written. The header shows Saved, Unsaved changes, Saving… or Not
  saved. Leaving the form, or opening another game, flushes first.
- **Ticks and takeaways save as they happen.** A tick is optimistic and
  reverts if the save fails.
- **A blank box means "not entered", never zero.** For deaths, that is what
  lets the form fall back to the death markers the recording counted.
  `reviewform/fields.ts` holds those rules, and the clear time's `m:ss` shape.
- **The spreadsheet is parsed here, not in the daemon.** The Objectives
  view reads the chosen CSV and `reviewform/sheet.ts` turns it into typed rows.
  The local timezone, DST included, is only known on this side, and it is
  what turns a spreadsheet's date and time into a moment. Unreadable rows are
  listed by line, and the rest are imported.
- **Both stores move on command results, not events.** No review event exists
  yet, because this form is the only writer. When P1 or P2 adds a second one,
  the stores should move to an event like everything else.

### The timeline stays above the fold

The review view is a page that scrolls, but the ruler under the advantage
curve is not optional furniture: it is how a position in the game is read off
the timeline at all, and having to scroll to it defeats the widget.

So the player is capped, and **the cap is a `max-width`, not a `max-height`**.
`.player-wrap` takes the vertical space left over after the app bar, the view
header and the whole timeline, and multiplies it by the recording's own aspect
ratio; `review.ts` publishes that ratio as `--player-ratio` on `loadedmetadata`,
falling back to the same 16/9 the video's `aspect-ratio` placeholder already
uses so the two never disagree before metadata lands.

Capping the height directly is the version that looks right and is wrong: the
element keeps its full width, `object-fit: contain` letterboxes inside it, and
resizing the window grows and shrinks black bars, which is why the original
`max-height: 60vh` was removed ([DEVELOPMENT.md §5.1](../DEVELOPMENT.md)).
Making the player narrower rather than shorter leaves nothing to letterbox.

Two details that are load-bearing rather than tidy:

- **`--player-chrome` is one number, and deliberately a little generous.** It
  is a sum of measured heights, and the app bar's is the one most likely to
  move. A rem too many costs a slightly smaller player; a rem too few puts the
  ruler back under the fold.
- **Fullscreen sets `max-width: none`.** The cap is about clearing a fold, and
  fullscreen has neither a fold nor a timeline beneath it; left on, it would
  clamp the video to a fraction of the screen.

On a tall window the cap never binds, because the content column's own width is
the smaller of the two, so this changes nothing for anyone who was not scrolling
in the first place.

### The player clips both ends

A recording brackets the game: it starts on the loading screen, twenty
seconds of a static splash, and it keeps rolling after the game window is
gone, which under WGC captures as *black*, not as a frozen last frame. Opening
a VOD used to land on the first, and playing one to the end used to land on
the second.

The review view treats the recording as a window instead,
`[game start − 1s, game end + 2s]`. Playback opens at its start, stops at its
end, the scrubber spans it, the ruler reads 0:00 at its start, and every seek
is clamped into it by a single `seekTo`.

**The file is untouched.** Both numbers come from the samples the timeline
already fetches: each carries a game clock and a video clock, so the offset
between them is the loading screen, and the last sample is the last thing the
game reported. No column, no migration, and it works on recordings made long
before this existed.

**Three ways it declines to clip the tail**, each falling back to the end of
the file rather than to a guess:

| Condition | Why |
|---|---|
| No samples | A rescan import, or a game whose poller never came up. Nothing knows where its game ended |
| Tail shorter than `MIN_TRIM_S` (3 s) | Not worth rewriting a gigabyte for. The tail margin is zero (DEVELOPMENT.md §5.4), so this is the only thing that keeps a short one |
| Gap wider than `MAX_TAIL_CLIP_S` (60 s) | Not a post-game tail. A stretch of unreadable Live Client Data responses keeps recording and produces *no samples*, so real gameplay would sit after the last one, and cutting there would hide the game |

Playback is stopped at the window end from both the rAF loop and
`timeupdate`: the loop is smooth but only runs while frames are produced,
`timeupdate` fires at ~4 Hz regardless. Whichever arrives first wins.

See [DEVELOPMENT.md §5.4](../DEVELOPMENT.md) for why the file is not cut, and
[#120](../../issues/120) for cutting the tail out of it.

### Where each kind of art comes from

| Kind | Source | Keyed by |
|---|---|---|
| Champion | Data Dragon | display name → key (`Wukong` → `MonkeyKing`) |
| Item | Data Dragon | the numeric id the game reports |
| Rune | Data Dragon | rune or tree id → an icon *path*, from an unversioned part of the CDN |
| Summoner spell | Data Dragon | display name *and* numeric id → art key (`Flash`, `4`, `74`, `2202` → `SummonerFlash`) |

Spells are the odd one out because `summoner.json` lists one entry per
game-mode *variant* rather than one per spell: `Flash` is `SummonerFlash`,
`SummonerFlash_Jade` and `SummonerCherryFlash`. `spell_art_map` collapses each
name onto the standard version, and maps every variant's id onto it as well, so
a live-captured scoreboard and one rebuilt from match history draw the same
picture from one cache file. Smite is folded the same way, since the jungle item
renames it `Primal Smite` mid-game and Data Dragon has no such entry. See
[DEVELOPMENT.md §5.3](../DEVELOPMENT.md).

### Art is asked for once per page, not once per icon

A row carries a champion portrait, two summoner spells, two rune icons, seven
item slots and ten more champion squares for the two team compositions. Forty
rows is therefore several hundred icons, and one IPC call each, every one a
CDN round trip the first time, would be a library that renders over several
seconds.

The team squares are the one set bounded by the *game* rather than by the
library: there are about 170 champions, a square is around 7 KB, and a library
of any size converges on the ones its owner actually meets. Ten times the names
asked for is not ten times the disk.

So `icons.ts` collects what the visible rows want, asks once (`resolve_icons`),
and caches the answer for the session. Misses are cached too: a champion Data
Dragon has never heard of must not be asked about again on every render.

`fillInArt` walks the rows eight at a time and paints each chunk as it lands,
so the top of the list fills in while the bottom is still resolving. The
backend fans out within a chunk as well, six icons at a time; see
[DEVELOPMENT.md §5.3](../DEVELOPMENT.md).

**The row is correct before any of it arrives.** Slots render empty and are
filled in afterwards, which is also exactly what an offline session gets
forever. The row still says the champion, the KDA, the CS and the result in
words. Nothing about the layout depends on a picture turning up.

Empty slots hold their place rather than collapsing. A build with four items is
a different thing from a game with no scoreboard, and a strip that shrank to fit
would say neither.

The spell-and-rune block fills **down each column** rather than across each row:
spells on the left, runes on the right, which is how every scoreboard in the
game arranges them. The markup order is therefore load-bearing: spell 1, spell
2, keystone, secondary tree.

**The rune column is the one exception to holding its place.** A game with no
rune page, which is every augment mode such as ARAM Mayhem (#281), stores no
`our_runes` at all, and two empty frames would draw a page that never existed.
`Loadout` leaves the column out instead. The perks track in `.vod-row` is a
fixed 38px, so the item strip beside it does not move.

The team block fills the other way, across each row, for the same reason: a
team is a line of five, so the line has to be what the eye picks up. Filling by
column there would interleave the two sides.

### The cluster tooltip stays inside the timeline

Hovering a glyph lists the markers it collapsed. Two things keep that list from
escaping the window, and neither can be expressed in CSS alone.

**Horizontally it is clamped in pixels.** `left` used to be the glyph's own
percentage, which with `translateX(-50%)` put half the box outside the track
for any glyph near either end, and the page grew sideways to contain it, so
the window became scrollable. `placeTooltip` clamps to the tooltip's offset
parent instead, which needs the *rendered* width and therefore has to run after
the content is in. A tooltip wider than the timeline is pinned left rather than
centred, because the start of a marker list is the part worth reading.

**Vertically it is capped and scrolls.** A dense teamfight clusters into a list
long enough to run off the top of the window. Past the cap it scrolls rather
than truncating: every marker in a cluster is one the user asked about by
hovering it.

That scroll costs something, and the cost is why `pointer-events` is
conditional. The tooltip overlaps the top of the track, so making it hoverable
unconditionally would put a dead strip over the glyphs beneath it. It takes the
pointer **only when the content actually overflows** and there is a scrollbar
worth reaching, and `hideClusterTooltip` then has to let the pointer move into
it, since leaving the glyph is what normally dismisses it and the tooltip is not
inside the glyph container.

### What a marker says

Two shapes, and the split is about who the marker is *about*.

**Kills name people**: `Killed Nautilus`, `Killed by Akali`, `Blitzcrank killed
Jarvan IV`. The name is the whole content: it is never yours, and it is what
you would scrub for.

**Objectives name nobody**: `Dragon`, `Baron`, `Herald`, `Turret`,
`Inhibitor`, `Ace`, `First Blood`. `classify_event` only writes one of these
when you took part. Every objective branch is gated on `took_part()`, and
`Ace` and `FirstBlood` on it being *you*, so the killer was always you or an
ally you assisted. Printing it told you your own champion's name, which is the
one thing you already know.

The elemental dragon type went the same way. It says which drake, not which
moment, and `Fire Dragon` followed by a champion name was four words to say
`Dragon`. **Elder is the
exception** and stays `Elder Dragon`: it is a different objective rather than a
flavour of the same one, and it is a thing you would go looking for by name.

`(stolen)` survives on the three it can apply to, because a stolen Baron is the
moment, not a detail.

### An event carries two clocks, and both are shown

Every marker stores `game_time_s` and `video_time_s`, and the event list and
the timeline tooltip print both: the game clock first, the position in the
recording after it in the subtler colour.

They are not the same fact twice. **The game clock is what the event *is*:**
Live Client Data's own number, recorded as the event arrived, and the one a
person says out loud, as in "Baron at 24:30". **The video time is derived**,
being `game_time_s` mapped through whatever alignment was in force at the time,
with a fallback for a marker seen before the game clock ever moved (see
`PendingMarker::resolve`, and [recording-pipeline.md](recording-pipeline.md)
for why the mapping happens at finalize rather than at ingest). It is the half
that can be wrong.

That is why both are shown rather than the more meaningful one alone. A
recording brackets its game, opening on the loading screen, so the two
never agree, and the gap between them *is* the lead-in. It is also the only
place a bad alignment is visible from the UI: a kill the list calls 24:30 that
seeks to black is a story the two numbers tell together and neither tells
alone. The Diagnostics panel names an unproven alignment
([dev-portal.md](dev-portal.md)); this is what it looks like from the front.

The game clock's colour is scoped to the list. The timeline tooltip is a fixed
dark surface in both themes with a palette of its own, and the page's muted
grey, chosen against a light background, disappears into it; scoped, the
tooltip's copy simply inherits the tooltip's own text colour.

## Backend communication

Two directions, deliberately asymmetric.

```mermaid
flowchart LR
    subgraph FE["Frontend"]
        S["status.ts"]
        M["main.ts"]
        L["library.ts"]
    end
    subgraph BE["Rust"]
        CMD["Tauri commands"]
        SUP["Supervisor"]
    end
    S -->|"pull: lcu_status + game_state_status<br/>setTimeout chain, interval scales with state"| CMD
    L -->|"pull: list_recordings, get_disk_usage, …"| CMD
    SUP -->|"push: library-changed event"| M
    UPD["update.ts"]
    UPDBE["update check<br/><small>lib.rs, every 6h</small>"]
    UPDBE -->|"push: update-status-changed"| UPD
    S -->|"on a state change only"| UPD
    M --> L
```

**Pull for live state.** The header's summoner/phase/recording readout comes
from a `setTimeout` chain, not `setInterval`: `lcu_status` reads a lockfile
and makes two HTTPS round trips, and a slow tick under `setInterval` would
stack calls on top of each other. The interval scales with game state, and
stretches to 10 s while the window is hidden.

Because that delay is only chosen when the *next* timer is armed, a
`visibilitychange` listener re-polls immediately when the window comes back.
Otherwise the header could show up to 10 s of stale state while the in-flight
timer ran out. The 60 s safety refresh is skipped entirely while hidden: it
rebuilds the whole grid with `innerHTML`, and `library-changed` already covers
real changes. Skipping it leaves its timestamp stale on purpose, so the first
poll after the window returns catches up at once.

**Push for the library.** `library-changed` is one of two backend→frontend
events: the supervisor emits it after a finalize, and `set_retention_policy`
after a deletion. Polling `list_recordings` instead would rebuild the grid
every few seconds and fight scroll and focus.

**Push for updates.** `update-status-changed` is the other. The background
check runs every six hours ([DEVELOPMENT.md §14](../DEVELOPMENT.md)), which is
far too slow to poll for. But *whether the offered update can be installed*
depends on game state, which changes constantly. So `status.ts` also nudges
`update.ts` on a state **edge** and not every tick: without it the Install
button would sit enabled through a whole game and only refuse at the click.

**The release notes are built as nodes, never as markup.** `latest.json` is
fetched over HTTPS but is *not* covered by the update signature; only the
installer it points at is. So everything in the notes is remote text the app
did not write. `renderNotes` therefore drops the `## What's changed` heading,
turns `- ` lines into a list, and makes a paragraph of anything else, with
every string reaching the DOM through `textContent`.

The one exception is emphasis, and it is an exception in *parsing*, not in
trust. GitHub's generated notes end with `**Full changelog**: <url>`, which a
pure-`textContent` paragraph rendered with its asterisks showing. `inlineNodes`
splits those runs into `<strong>` elements built with `createElement` and
filled with `textContent`, so nothing from the manifest is ever interpreted as
HTML. Emphasis is the only inline syntax the notes contain, so it is the only
one handled. An unclosed `**` matches nothing and the line shows as written,
which is the same "shown rather than swallowed" rule the line types follow.

### Command surface

The names and arguments below are the IPC contract and have not changed, but
how they reach Rust has. `bridge.ts` sends all but three of them through a
single `rpc` command, `invoke("rpc", { command, args })`, which `core`'s
dispatch table routes by name
([DEVELOPMENT.md §12](../DEVELOPMENT.md#12-process-model-a-recorder-daemon-and-a-ui-that-can-leave)).
Callers are unaffected: `call()` takes the same name and the same args object,
and forwards the args untouched.

The exceptions are in `DIRECT_COMMANDS` in `bridge.ts`:
`open_recordings_folder` and `dev_open_portal` drive the desktop shell, so they
stay in the UI process; `dev_registered_commands` must stay direct because
`devportal.ts` detects the portal's existence by watching that call *reject* in
a shipped build.

> **`src/types.ts` is no longer the source of truth either.** Since WS2.2 all
> 57 types crossing the boundary derive `ts_rs::TS` beside their serde derives,
> and WS2.5's generator emits the TypeScript from those. The hand-written
> interfaces in `src/types.ts` are what that replaces.
>
> They agree today, checked rather than assumed: the generated `RecordingRow` uses
> `number` for every `i64`, which is what this file has said since v1 and what
> `JSON.parse` actually produces. ts-rs's *default* would have said `bigint`,
> which JSON cannot carry at all; `contract::types::config()` is where that is
> corrected and a test pins it.

> **This whole table is now generated, and `src/lib/contract/` is the output.**
> Since WS2.5 `cargo run --features contract-gen --bin gen-contract` emits
> `types.ts`, `client.ts`, `events.ts` and a barrel from the Rust declaration,
> and CI runs the same binary with `--check`, so a command added without
> regenerating cannot merge.
> The generated files are committed, which is what lets a frontend developer
> work without a Rust toolchain and makes a contract change reviewable as a
> diff. Biome does not format them: a formatter rewriting a generator's output
> is a loop.
>
> There is deliberately **no Rust-to-TypeScript type map in the generator**.
> The macros resolve the names where they still hold the real types, so
> `dispatch_table!` emits a manifest whose types are already rendered and every
> boundary type answers `ts_rs::TS::decl`. A third list able to disagree with
> the other two is the thing this workstream exists to delete.
>
> The generated client's **method name is the wire name**, `list_recordings`
> rather than `listRecordings`. Only the *arguments* are camelCased, because
> that is the only rename serde actually performs, and camel-casing the method
> too would add a second mapping to keep in step.

> **The `Returns` column is no longer the source of truth.** Since WS2.1 every
> row of `dispatch_table!` declares its own return type, and the compiler checks
> the declaration against the function, so a wrong one is an `E0308` at the `?`,
> not a silent disagreement. `core::dispatch::contract_manifest()` is where that
> lives now, and WS2.5's generator emits the TypeScript from it.
>
> This table survives because it carries the one thing the manifest does not:
> **which part of the UI uses each command.** Keep that column accurate. If the
> `Returns` column and the manifest ever disagree, the manifest is right, and
> WS2.5 deletes this column rather than fixing it.

| Command | Returns | Used by |
|---|---|---|
| `list_recordings` | `Vec<RecordingRow>` | library grid |
| `rescan_recordings` | `ReconcileReport` | library toolbar → rescan |
| `backfill_match_metadata` | `BackfillReport` | settings → storage → fill in |
| `resolve_icons` | `IconSet` | library row art, after the list paints |
| `get_recording_markers` | `Vec<MarkerRow>` | review timeline |
| `get_recording_samples` | `Vec<SampleRow>` | advantage curve |
| `get_disk_usage` | `DiskUsage` | library stats bar |
| `get_retention_policy` / `set_retention_policy` | policy / `EnforcementReport` | settings → storage |
| `preview_retention_policy` | dry-run deletion list | settings, while editing |
| `set_pinned` | nothing | library 📌 |
| `delete_recording` | nothing | library card |
| `get_recordings_dir` / `open_recordings_folder` | path / nothing | settings |
| `get_ui_prefs` / `set_ui_pref` | `HashMap<String,String>` / nothing | `prefs.ts` |
| `get_autostart` / `set_autostart` | `AutostartStatus` | settings → background & tray |
| `get_audio_preset` / `set_audio_preset` | `AudioPreset` / nothing | settings → audio |
| `list_audio_inputs` | `Vec<AudioInputDevice>` | settings → microphone picker |
| `get_capture_backend` / `set_capture_backend` | `CaptureBackendStatus` | settings → advanced → capture backend |
| `extract_audio_track` | path to a cached sidecar | review player, stem selection |
| `lcu_status` | `LcuStatus` | header strip |
| `game_state_status` | `SupervisorStatus` | header strip, About block |
| `dev_open_portal` | nothing | the header's dev button, and the 🔎 on each library row (which passes a `recordingId` so the portal opens on it) |
| `get_update_status` | `UpdateStatus` | settings → About (version + changelog), and the badge on the gear |
| none | the `updateChannel` pref | the channel dropdown rides `get_ui_prefs`/`set_ui_pref`, so it needs no command of its own |
| `check_for_update` | nothing | settings → About → "Check now" |
| `install_update` | nothing | settings → About → "Install and restart"; ends the process |
| `start_recording` / `stop_recording` / `is_recording` | nothing | registered but unreferenced by the main UI, and since #282 by the dev portal's panels too: they reach the daemon's recorder around the supervisor ([dev-portal.md](dev-portal.md#the-recorder-panel-has-no-manual-controls)). Only the Commands panel can still invoke them |
| `open_game_for_recording` | the game's id | review form, opened from a library row; makes the game for a recording from before WS9 |
| `get_game_review` / `save_game_review` | `GameReview \| null` / nothing | review form: load, then the debounced autosave of the whole `ReviewInput` |
| `set_objective_ticked` | nothing | review form → "Reviewing against" |
| `add_takeaway` / `delete_takeaway` / `promote_takeaway` | `Takeaway` / nothing / `Objective` | review form → Takeaways |
| `list_objectives` / `create_objective` / `update_objective` / `set_objective_status` | `Vec<Objective>` / `Objective` | Objectives view |
| `split_block` / `merge_blocks` | new block id / nothing | nothing in the UI yet: the block view is WS9 P3. The dev portal's Commands panel drives them |
| `import_review_rows` | `ImportReport` | Objectives view → "Import spreadsheet…", with the rows `reviewform/sheet.ts` parsed from the chosen CSV |

**Start on login is the one setting that is not a pref.** It lives in the
platform's own store (`HKCU\…\Run` on Windows) which the user can also edit
from Task Manager, so `settings.ts` reads it from `get_autostart` when the view
loads instead of from the `prefs.ts` cache, and applies whatever
`set_autostart` reports *back* rather than the value it just sent
([DEVELOPMENT.md §12](../DEVELOPMENT.md#12-process-model-a-recorder-daemon-and-a-ui-that-can-leave)).
It is one of two rows in the settings form that can come back disabled, when
the build has no autostart control or the read failed.

**The capture backend is the other.** `Settings.svelte` renders the
Advanced group, which holds only this row, in every build. It was
devtools-only until #243 made the own backend the default and deleted the
`hasDevCommands()` gate in the same change: libobs is now the fallback a
release user can pick without a reinstall
([DEVELOPMENT.md §16](../DEVELOPMENT.md#the-switch-and-when-it-applies)).
`Settings.test.ts` checks that it renders without the dev commands.

`capture_backend` is a `settings_kv`
key, but it is not read through `prefs.ts` and not written with
`set_ui_pref`, because writing the row is not the change: the daemon has to
check the backend can be built here, refuse while a game is in progress, and
swap the live recorder
([DEVELOPMENT.md §16](../DEVELOPMENT.md#the-switch-and-when-it-applies)).
**Automatic, when nothing is saved.** `CaptureBackendStatus` carries
`automatic: true` when no `capture_backend` row exists, and `configured` is
then the daemon's pick: Own where it can be built, else libobs. The row says
so in plain text (`settings/capture.ts`'s `automaticNote`): "Automatic: Own,
the default." or "Automatic: libobs, because <Own's reason>.". Only a click
writes the row, and clicking the automatic pick does write it:
`saveCaptureBackend` skips a click only on a backend that is already saved.

`Advanced.svelte` renders what `get_capture_backend` reports: every backend
the daemon knows about, each with a one-line explanation
(`settings/capture.ts`'s `BACKEND_EXPLAINED`: Own is the default, libobs the
fallback for one release), one it cannot build **disabled with the daemon's
reason** beside it (the own backend off Windows or below build 20348), and
the backend actually
live. A click calls `set_capture_backend` and the control shows the status it
returns, never the value it sent, so a refusal leaves it where it was and
raises a toast. The row says that a change applies from the next recording.

**The software-encoding notice.** When `CaptureBackendStatus` has
`software_encoding` set, the row shows a warning callout
(`settings/capture.ts`'s `softwareNote`) that recording is encoding in
software and costs more CPU: the UI's third of DEVELOPMENT.md §2.4's rule that
the own backend's software fallback is never silent. The own backend only
learns its encoder when the League client opens, long after the view first
reads the status, so `status.svelte.ts` calls `refreshCaptureBackend` on every
game-state edge, next to `refreshUpdateStatus`; it does nothing until the view
has read the status once. The daemon keeps the flag set after the client
closes, so the notice is still there when Settings is opened after a game.
Every string in it that came from the daemon is interpolated, never rendered
as markup.

### Event surface

The other half of the contract, and the half v1 never declared. Commands are
the UI asking; events are the daemon telling. Until WS2.3 the second kind
existed only as string constants at each `Emitter::emit` call site
(`LIBRARY_CHANGED_EVENT`, `UPDATE_STATUS_EVENT`), mirrored by hand in
TypeScript.

`contract::events`'s `contract_events!` table is now the single declaration.
Each row is `Variant { field: Type, … } => Topic`, and the enum, `topic()`,
`event_names()` and `event_manifest()` are all generated from it, so a variant
cannot be added without a topic, because that is a syntax error rather than a
convention.

> **Appendix B calls this "a small derive"; it is a `macro_rules!` instead.** A
> real derive needs a proc-macro crate, which would mean making `src-tauri` a
> workspace and taking `syn`, `quote` and `proc-macro2` to produce output a
> declarative macro produces already. `dispatch_table!` is the precedent: it
> declares the command half and emits its manifest beside it. The plan's output
> is unchanged; only the mechanism is.

Events are internally tagged, so one arrives as
`{"type":"stateChanged","state":"Recording","sinceMs":…}` and a generated
client gets a discriminated union it can switch on exhaustively.

**A client subscribes by topic, never by event name.** That is what lets a
variant be added to an existing topic without a client change.

| Topic | Carries | Subscribed by |
|---|---|---|
| `recording` | `StateChanged`, `RecordingStarted`, `RecordingStopped`, `MarkerAdded`, `SampleBatch`, `CaptureProblems` | main UI, dev portal |
| `lcu` | `LcuPhase` | main UI, dev portal |
| `library` | `LibraryChanged`, `MatchSummaryPatched`, `RetentionRan` | main UI, dev portal |
| `update` | `UpdateStatus` | main UI, dev portal |
| `daemon` | `DaemonShuttingDown`, `Lagged` | dev portal; the transport handles `Lagged` itself by re-`hello`ing |

Three details are load-bearing:

- **`SampleBatch` is a batch on purpose.** A 35-minute game produces about
  2,000 advantage samples, and 2,000 notifications is a frame-rate problem in
  the UI for data drawn as one line.
- **`recording_id` is `Option<i64>` while a game is in flight.** The library
  row is written at finalize, so nothing has an id until then; a client
  correlates on `RecordingStarted.file_stem` until `RecordingStopped` names the
  row. That field is the stem rather than a path because `Recorder::start`
  returns `Ok(())`, so the file it wrote, extension included, is only known when
  `stop` returns.
- **`LcuPhase.phase` is `None` exactly when no client is running**, which is a
  different statement from `GameflowPhase::None`, which is a client sitting at
  the front page. A test pins the two apart.
- **`CaptureProblems` is once per recording, and only for a failure** (#10). It
  follows `RecordingStopped` with the row's id, or stands beside a crashed one
  with no id when there is no recording (`notStarted`, `notSaved`). It carries
  the Windows build and a list of tagged `CaptureProblem`s whose `reason` is a
  Windows call's own message: untrusted text. A source the preset names that
  was simply not there is not a failure and is never in it (DEVELOPMENT.md
  §2.6). It is the fourteenth event; the other addition past Appendix B's
  twelve is `ShowUi`.

**Q7 is answered by the code rather than left open.** Issue #73 asks whether
`LcuPhase` carries the full `gameflow-phase` enumeration or "the subset
`lcu/gameflow.rs` models today". There is no subset: `GameflowPhase` already
names all fourteen phases the LCU defines and carries `Unknown(String)` for
anything a client update invents. The state machine *consumes* a subset, in
that `is_game_running_phase` matches two variants, but that is a reader
narrowing a full type, not a narrow type. So the event carries it whole.

### What emits them

`Supervisor::set_event_sink` installs the sink, from `lib.rs` and for the same
reason `set_event_notifier` is installed there: `run()` is dead code in a `cargo
test` build and gets stripped, which is what keeps Tauri's Wry window machinery,
and the whole Win32 GUI import stack behind it, out of the test binary. The
sink is a type-erased boxed closure, never an `AppHandle`
([supervisor.rs](../src-tauri/src/state_machine/supervisor.rs)'s `on_event`
comment records what happens when that rule is broken: the test binary died at
load with `STATUS_ENTRYPOINT_NOT_FOUND` before running a single test).

Everything goes out on one Tauri event, `event`, because the contract's own
discriminant is `type` and that is what a client switches on. The v1 channels
(`library-changed`, `update-status-changed`) still fire beside it; WS2.7 deletes
them.

| Event | Emitted from | Wired |
|---|---|---|
| `StateChanged` | `dispatch_one`, on transitions that actually move | yes |
| `LcuPhase` | the gameflow watcher, and `stop_gameflow_watch` for `client_present: false` | yes |
| `RecordingStarted` / `RecordingStopped` | `start_recording` / `stop_recording` | yes |
| `MarkerAdded` | `on_snapshot`, one per marker a poll produced | yes |
| `LibraryChanged` | `emit_library_changed` | yes, `Finalized` and `Edited` |
| `RetentionRan` | the post-finalize enforcement pass | yes |
| `UpdateStatus` | `record_update_result` in `lib.rs` | yes |
| `SampleBatch` | nothing yet | no: the 5 s window needs a *subscriber* to batch for (WS2.6) |
| `MatchSummaryPatched` | nothing yet | no: published from the `library-changed` site still in `lib.rs` |
| `DaemonShuttingDown`, `Lagged` | nothing yet | no: they describe a daemon and a broadcast buffer WS3 builds |

Declaring the unwired four is the point rather than an oversight: the contract
is what the surface *is*, not what happens to be connected. They carry
`cfg_attr(not(test), allow(dead_code))` meanwhile, as `CLAUDE.md` describes.

**`SupervisorEvent` is not a second declaration of the wire.** It is internal:
it triggers desktop notifications and the v1 `library-changed` push, it carries
what a toast needs to describe itself, and it is deliberately absent from
`contract::types`' boundary list because it never crosses IPC. One declaration
of what crosses, one internal callback for what does not. WS3 folds them
together when notifications move into the daemon.

**A marker published live is provisional.** `video_time_s` comes from the
alignment known at that poll, and a later poll can improve it, so a marker on
the wire mid-game can sit a fraction of a second from where the same marker
lands in the database, which resolves every marker against the final alignment.
The live value draws a timeline while the game runs; the row is what the library
reads.

### The snapshot, and why it is not a replay

Commands answer questions and events announce changes, which leaves one gap: a
client that has just connected knows nothing, and a client that reconnects after
a dropped pipe knows something out of date. `contract::snapshot::Snapshot` is
the answer to both. A client gets exactly one, then a stream of events, and it
**never replays history**. There is no backlog to drain, so there is no way to
be subtly behind.

| Field | Kept current afterwards by |
|---|---|
| `state` | `StateChanged` |
| `lcu` | `LcuPhase` |
| `currentRecording` | `MarkerAdded`, `SampleBatch` |
| `update` | `UpdateStatus` |
| `prefs` | nothing; the UI writes these and owns the echo |

The snapshot and the event surface describe the same state deliberately. A
field with no event would go stale without saying so, and an event with no
field would leave a reconnecting client with no baseline to apply it to.

**`seq` positions it in the stream.** An event numbered at or below it is
already reflected in the snapshot and is dropped; the first event a client
applies is `seq + 1`. Without that, a snapshot assembled while events were in
flight would be applied over newer state. The counter belongs to the transport
(WS3.1), so `assemble` is handed one rather than inventing it.

**`lcu` is handed in for a sharper reason.** `core::lcu_status` is async,
discovers the lockfile itself and makes two HTTPS round trips; its own doc calls
it a smoke test. Putting that on every connect would make the handshake cost a
network request, and would re-derive what the state machine already knows from
`lcu::gameflow::watch`.

**`state` is the bare `GameState`, not `SupervisorStatus`.** The latter also
carries `last_finalized`, which is a whole recording including every marker of
the last game. A message sent on every connect must not grow with the length of
a game, and "what did the last game produce" is a library question that
`list_recordings` and `get_recording_markers` already answer. What is live
rather than stored is in `currentRecording`, which carries the file stem, the
running marker and sample counts, and the alignment offset (`null` until the
game clock is first seen to advance, which is not the same as `0`).

Nothing sends one yet: WS3.1 is where a pipe exists to send it down. The type is
declared now for the same reason the four unwired events are, which the table
above records.

## Routing and the tray

`router.ts` owns which view is showing. `initRouting` adds two entry points the
tray needs: a `#settings` URL fragment read once at startup, for a window the
tray has just created, and a `navigate` event for a window that already exists.
A `#review` fragment is ignored, because the review view with no recording
loaded is not a state worth restoring into. `#game` is ignored for the same
reason; `#objectives` opens that list.

## The root, and how it mounts

WS4 was a strangler rather than a rewrite, so for its length the window ran two
frontends at once: the sections `index.html` still held, and one Svelte root
mounted beside them. There is one frontend now, and the arrangement that made
the crossing safe is the arrangement that remains.

`router.ts` owns the join, because it already owned the only question the two
halves had to agree on, which is **which view is showing**. A second module
toggling `hidden` is the exact failure `showView` was written to end, and a
Svelte root that hid its own siblings would have been that failure with a
compiler in front of it.

| Export | What it does |
|---|---|
| `mountApp(component, host)` | brings the root up, once; a second call is refused rather than duplicating the tree |
| `unmountApp()` | takes it down, resolving `true` if there was anything to take down |
| `appMounted()` | whether it is currently up |

**A view registers itself.** There is no markup to look up any more, and a
Svelte view has no id to look up before it renders, so `App.svelte` hands its
node to `registerView` on mount. That
happens *after* `initRouting` has read the URL fragment, which is why
`registerView` sets `hidden` from the current view rather than trusting the
node's default: a window opened at `#settings` would otherwise show the
settings section and the library at once.

**The root goes up once and stays up.** It is deliberately not driven from
`showView`: `mount` and `unmount` destroy component state, so tying them to
view changes would throw away a view's scroll position and in-flight requests
every time the user glanced at Settings. Visibility stays what it has
always been, the `hidden` attribute on the host. `unmountApp` exists so that
mounting is reversible, which is what makes the root testable; nothing calls it
in normal use.

`main.ts` mounts last, after every `init*` has run, so a component throwing on
the way up cannot take a working frontend down with it.

### What the library looks like now

`library.ts` is gone. 777 lines of it, plus the `#library-view` markup, are
eight components, a store and the pure modules WS4.2 extracted.

```
lib/stores/library.svelte.ts   the row set, the disk figure, every control
lib/stores/icons.svelte.ts     when art has arrived
lib/components/library/
  Library.svelte               the view: stats, toolbar, grid, empty states
  StatsBar.svelte              the four numbers
  Toolbar.svelte               search, facets, outcome, pinned, sort, actions
  Row.svelte                   one recording
  Loadout.svelte               spells, runes, items
  Matchup.svelte               the lane opponent
  RowActions.svelte            pin, delete, inspect
  Slot.svelte                  one art box
```

**Four things went away rather than being ported.**

`render()` and `pendingRender`. The old module recomputed the visible rows and
rebuilt the grid with `innerHTML` whenever anything changed, and had to defer
that by hand while another view was showing, because rebuilding a hidden grid
is wasted work and doing it as the user navigated back would yank the card they
came from out from under them. Svelte does not render a component that is not
mounted.

`paintArt` and `paintAll`. Art is a second pass, still, for the reason it
always was: the first sighting of any icon is a CDN round trip, and the whole
thing has to work with no network at all. But `innerHTML` used to throw away
every image on every re-render, so the old code re-queried the grid by
`data-id` and re-inserted `<img>` tags by hand. Now `Slot` reads the cache and
`icons.svelte.ts` publishes a version counter when it grows.

`revealRowInspectors`. The dev-portal probe resolves once and the grid was
rebuilt many times, so its answer had to be remembered and re-applied by
querying every `[data-inspect]` button. `Library.svelte` asks `hasDevCommands`
itself and passes the answer down, so the button is not rendered rather than
rendered and hidden.

`escapeHtml` / `escapeAttr` on this path. Every one of the row's values used to
be concatenated into a template string with an escape applied by hand at each
site, and `vodTitle` falls back to a filename, which is untrusted input.
Default interpolation replaces both. **`{@html}` anywhere would opt straight
back out of it**, which is what `Row.test.ts`'s last two tests and
`markup-guard.test.ts` exist to catch. See
[Escaping](#escaping-and-why-there-is-nothing-left-to-call).

One thing deliberately did not change: a row is a focusable, clickable
`role="listitem"`, which is what v1's markup did and what the a11y warnings in
`Row.svelte` are suppressed for. ARIA has no good role for an activatable list
item, and making every row contain a real button is a UX change rather than a
migration. It is flagged in the component and worth its own issue.

### What stayed vanilla, and why

WS4.6 deleted the last modules that owned an element. `index.html`'s body is
one `<div id="app-root">`; `App.svelte` renders the app bar, the daemon strip,
the three views, the quit dialog and the toast.

| Was | Is |
|---|---|
| `toast.ts` | `stores/toast.svelte.ts` + `shell/Toast.svelte` |
| `daemon.ts` | `stores/daemon.svelte.ts` + `shell/DaemonStrip.svelte` (and, new in #10, `stores/capture.svelte.ts` + `shell/CaptureStrip.svelte` beside it) |
| `quit.ts` | `stores/quit.svelte.ts` + `shell/QuitDialog.svelte` |
| `status.ts` | `stores/status.svelte.ts` (the poll kept; the pills go to a store) |
| `appbar.svelte.ts`, `devportal.ts` | `shell/AppBar.svelte` |
| `dom.ts` | deleted; its escape helpers moved to `src/dev/ui.ts` and went with that in #72 |

Three modules stay vanilla and are meant to: `main.ts` composes, `theme.ts`
owns `html[data-theme]`, and `desktop.ts` suppresses browser behaviours at the
document level. None of them is a view and none of them looks up an element.
Beside them sit the shared libraries the components import: `bridge`, `format`,
`icons`, `prefs`, `router` and `types`. They are modules rather than markup and
were never what "vanilla" meant here.

**The one deliberate exception in `<head>`.** The anti-flash boot script has to
run before first paint, and so does the stylesheet whose tokens it depends on,
so both stay a plain `<script>` and a plain `<link>`. A stylesheet that arrived
over a JS import would be the flash the script exists to prevent.

#### The stylesheet moved; it did not scatter

`src/styles.css` is `src/lib/styles/app.css` and is still one global sheet.
**The remaining half of WS4.6 is not done and should not be assumed done.**

Svelte scopes a component's `<style>` to that component's own template, so a
rule written in one component and matching an element another renders silently
stops applying. `.vod-items .vod-slot` is the shape of the problem: the
positioning belongs to `Loadout`, the box belongs to `Slot`, and distributing
them needs `:global()` in exactly the places where it is easy to be wrong. With
1,900 lines of that, no visual test anywhere in this repo, and no way to look
at the result, it is a change whose failure mode is invisible to whoever makes
it.

Doing it component by component, each verified against a running window, is the
way it should happen.

### The player is an island, and the island has a coastline

WS4.5 deleted `review.ts`. What replaced it is not the same shape as the other
views, and that is the point of migrating it last.

**A `<video>`'s position is not state.** `currentTime` changes sixty times a
second while playing, the element is its own source of truth, and every seek is
a command rather than an assignment. So `Review.svelte` holds a real element
reference and talks to it directly, exactly as `review.ts` did: the rAF
playhead loop, the pointer-capture scrubbing, the stem `<audio>`, fullscreen
and the visibility pause are all imperative and stay that way.

What changed is that the island now has a defined coastline.

| Crosses the boundary | Direction |
|---|---|
| `playhead` (one number, from the rAF loop) | out, to `Timeline` |
| `review.window` | in, from the store |
| `markers`, `samples`, `metric` | in, from the store |
| `seekTo`, `onscrub` | in, as callbacks |

One number leaving per frame is the smallest seam that still lets the timeline
be declarative, and the timeline is the part that benefits most: its five graph
states used to be five early returns writing different text into three
different elements, and are now one `graphView` call returning which of five to
render.

**The store holds the recording, never the player.** `currentTime`, `paused`,
`volume`, the selected track and the fullscreen state all live on the component
beside the elements that own them. Putting any of them in the store would be
claiming they are shared, and nothing else has any business reading them.

Two small corrections fell out of testing. `seekTo` and `togglePlay` both
guarded on `Number.isFinite(video.duration)`; they guard on
`review.window.span > 0` now, which is the same fact read from the side that
everything else in the component already reads, and is the reason an empty
window makes them no-ops rather than seeking to zero.

The `[` / `]` / `d` / `D` hotkeys are **the only marker navigation available in
fullscreen**, because the rich timeline is outside `.player-wrap` and is
therefore not rendered there. `lib/review/hotkeys.ts` is the decision and
`Review.svelte` acts on it, which is what finally made the guards testable: a
focused `<button>` must not swallow the arrows, because clicking a timeline
glyph leaves one focused.

### Settings, and the three things that outlived it

WS4.4 deleted `settings.ts` and `update.ts`. What made it more than a second
helping of WS4.3 is that the settings markup had **four** owners, not one.

| Wrote into `#settings-view` | Now |
|---|---|
| `settings.ts` | `lib/components/settings/`, eight components |
| `update.ts` | `Update.svelte` and `UpdateNotes.svelte`, over `lib/stores/update.svelte.ts` |
| `status.ts` | still polls; publishes three lines to `lib/stores/about.svelte.ts` |
| `index.html`'s app bar | `appbar.svelte.ts`, which is not a view |

**`status.ts` only lost its element references.** It owns the poll timer and
the app bar's two pills, and always has; what it had also acquired was three
rows of a view it has nothing to do with, written by reaching into
`#about-lcu` and friends. The wording moved to `lib/settings/about.ts` where it
can be tested, and the values now go to a store. The poll is untouched.

**`appbar.svelte.ts` exists because the header is not a view.** The settings
button and the update dot live in `index.html`, which WS4.6 deletes and WS4.4
does not. The alternative was keeping `update.ts` alive to toggle one
element's `hidden`. It uses `$effect.root`, which is what lets a plain module
read a rune; the root is never torn down, which is right for elements that
live as long as the window.

**Preferences are mirrored, not moved.** `prefs.ts` still owns them, still
writes through `savePref`, and still keeps the localStorage cache the boot
script reads before first paint. The store holds a reactive copy so controls
can bind to it, and `syncFromPrefs` fills it in when SQLite answers, which is
the same moment `settings.ts` used to re-apply every control by hand.

**`theme.ts` is untouched on purpose.** It owns `html[data-theme]` and the
matchMedia `change` listener that makes "System" follow the OS live.
`Appearance.svelte` asks it to change and does not write the attribute itself:
a second writer would be racing the listener. CLAUDE.md names removing that
listener as a silent regression with no test to catch it, which is still true.

The one thing jsdom forced: `matchMedia` has no implementation there, and
`theme.ts` calls it at module scope, so any test reaching the settings view
failed at import. The fix is `src/test-setup.ts`, a shim for the environment,
rather than moving a call that is load-bearing.

#### What `test-setup.ts` shims, and the one that is opt-in

`matchMedia`, `ResizeObserver`, `<dialog>`'s `showModal` and `close`, and an
inert `HTMLMediaElement`. Two of those are worth knowing about:

**`showModal` is overridden unconditionally.** jsdom *defines* it and then
throws from it, so a `typeof` guard installs nothing and the dialog silently
never opens.

**`ResizeObserver` never fires on its own, and `resizeTo` is how a test makes
it.** Every element in jsdom is zero pixels wide, so an observer that invented
a callback would be inventing a layout, and every assertion downstream would
be measured against a number nobody chose. `resizeTo(element, width)` sets the
property and notifies whoever is observing it, which is what `bind:clientWidth`
needs, and it goes **in the test**, next to the assertion that depends on it.
That is the difference between stating an assumption and hiding one. It is
also what makes the marker glyphs reachable at all: `clusterMarkers` measures
in pixels and correctly returns nothing at zero width, so before it there was
no way to test the cluster tooltip through the component.

### The pure logic comes out first

WS4.2 moved the decisions out of `review.ts` and `library.ts` ahead of the
views that replaced them, and every view after it did the same on the way
across. Nothing changed behaviour; what changed is that all of it is reachable
without a DOM.

| Module | What it owns |
|---|---|
| `timeline/window.ts` | which slice of the file is the game: the lead-in skip, the tail clip and its three refusals, and every fraction drawn along the timeline |
| `timeline/clusters.ts` | markers too close together to draw separately, and which icon a cluster shows |
| `timeline/graph.ts` | the advantage curve's max-abs downsampling, and the ruler's tick spacing |
| `timeline/stem.ts` | whether a drifting audio stem is nudged back or seeked |
| `timeline/markers.ts`, `timeline/navigate.ts` | what a marker is labelled, and which one a jump lands on |
| `library/filters.ts` | which recordings the library shows, and whether anything is narrowing it |
| `library/sort.ts` | facet ordering, and the row order the sort control picks |
| `library/stats.ts`, `library/scoreboard.ts` | the header totals, and the lane opponent a row can name |
| `review/hotkeys.ts`, `review/playback.ts` | which key means what, and the seek and rate arithmetic |
| `settings/` | the release-note parse, the backfill and retention wording, the audio device list, and every About line |
| `dev/` | the portal's routing, formatting, argument coercion, log filtering, cell parsing and seed presets |

These were not untested by oversight. Every one of them read module-level
mutable state or a form control, so asking "does a pinned-only filter hide an
unpinned row" meant building a settings form, and exercising the tail clip
meant constructing a review view with a loaded video. Taking that state as
arguments is the whole of the change.

Two things the extraction is careful about. `library.ts` still owns the
controls: it reads them into a `LibraryFilters` and hands that over, so
nothing under `lib/` knows an element exists. And `vodTitle` is injected into
the filter predicate rather than imported, so the champion box can be tested
against a title that says what the test means instead of against the
fallback-to-filename rules, which have tests of their own.

**Coverage is scoped to `src/lib/`** and floored at 80% by
`npm run coverage`. It was scoped that way while the number would otherwise
have been dominated by the DOM wiring being strangled; it stays that way
because `src/` outside `lib/` is now four composition modules and a mount,
and `main.boot.test.ts` covers the only claim they make.

### What has moved across

| View | Lands in | State |
|---|---|---|
| `App.svelte` | WS4.1 | landed |
| `Library.svelte` and children | WS4.3 | **landed**; `library.ts` deleted |
| `Settings.svelte`, `Update.svelte` | WS4.4 | **landed**; `settings.ts` and `update.ts` deleted |
| `Review.svelte`, `Timeline.svelte` | WS4.5 | **landed**; `review.ts` deleted |
| the shell, and `index.html`'s body | WS4.6 | **landed**; `dom.ts`, `toast.ts`, `daemon.ts`, `devportal.ts` and `appbar.svelte.ts` deleted |
| `DevApp.svelte` and eleven panels | #72 | **landed**; `src/dev/main.ts` rewritten to a mount, `src/dev/ui.ts` and `src/dev/panels/` deleted |

Each of those deleted its vanilla counterpart and the markup `index.html` held
for it in the same commit, so no view was ever owned by both. The player went
last and stays an imperative island: it owns real DOM nodes, because
`<video>` `currentTime` is not state anything should be diffing.

WS4.7 closed the workstream with the three things that only make sense once
everything has landed: the structural `{@html}` guard, a Biome-clean tree, and
this document rewritten around the component tree rather than the crossing.

The dev portal (`dev.html`, `src/dev/`) was out of scope under plan §9, Q6,
which offered retiring it. It was reworked instead (#72): it is eleven Svelte
panels now, and the last hand-built markup in the repo went with it. See
[docs/dev-portal.md](dev-portal.md).

### Two type gates, not one

`tsc` does not look inside `.svelte` files at all. `npm run check:svelte` is
therefore a separate CI gate from `npm run typecheck`, and it landed with the
first component rather than after it: without it the type gate would narrow
silently, view by view, as the migration proceeded. See
[docs/ci-and-releases.md](ci-and-releases.md) for why neither is spelled
`npx tsc`.

One consequence worth knowing before it costs an afternoon: **a `.svelte` file
needs a `<script>` block even when it is empty.** `svelte2tsx` emits a typed
component for a file that has one and an untyped one for a file that does not,
so deleting an empty block turns the importing module's `import` into an
implicit `any`. The error is reported against the importer and never mentions
the component.

## Theming

Every custom property lives in `src/lib/styles/tokens.css`, which `styles.css`
pulls in with an `@import` on its first line. WS4.1 moved them there unchanged,
values, selectors and cascade order alike, so that the components have a token
source that does not disappear when WS4.6 deletes the stylesheet. **Nothing
else should load that file.** A second `<link>`, or a JS `import` inside a
component, arrives after first paint, which is the theme flash the inline boot
script exists to prevent.

`data-theme` on `<html>` is written by JS and only ever holds `"light"` or
`"dark"`, and there is no `prefers-color-scheme` query in the stylesheet.
Resolving the OS preference once, in one place, keeps a single dark block
instead of two and makes an explicit "Light" on a dark OS win by construction
rather than by CSS specificity.

The cost: "System" no longer follows the OS for free. `theme.ts` listens on
the matchMedia `change` event to put that back, and **removing that listener is a
silent regression with no test to catch it.**

```mermaid
flowchart LR
    A["settings_kv (SQLite)<br/><small>source of truth</small>"] --> B["prefs.ts cache"]
    A -.mirror.-> C["localStorage"]
    C --> D["inline boot script in index.html<br/><small>picks a theme synchronously,<br/>before first paint</small>"]
    B --> E["theme.ts → html[data-theme]"]
    F["matchMedia change"] --> E
    D --> E
```

`localStorage` exists for exactly one reason: the boot script has to choose a
theme before first paint and IPC resolves too late. SQLite stays the source of
truth and wins any disagreement.

## Review player

- A plain `<video>` element. H.264/AAC MP4 decodes natively in the webview, so
  seeking and playback rate come for free.
- Video loads through Tauri's asset protocol (`convertFileSrc`), scoped in
  `tauri.conf.json` to `$APPDATA/recordings/*` and
  `$APPDATA/recordings/audio-tracks/*`. This needs the `protocol-asset` Cargo
  feature, not just the config entry. The second entry is not redundant:
  Tauri's scope matcher won't let `*` cross a `/`.
- **The controls live inside `.player-wrap`**, over a scrim at the bottom of
  the video, not in a bar beneath it. That is not cosmetic:
  `requestFullscreen` is called on `.player-wrap`, and anything outside the
  fullscreened subtree is not rendered at all, so controls beside the video
  simply vanished when you pressed `f`. The `:fullscreen` rules in
  `styles.css` are load-bearing for the same feature: without them
  `#review-video` keeps its `max-height: 60vh` and renders as a small
  rectangle in the middle of a black screen.
- **Two seek surfaces, one implementation.** The plain progress bar inside the
  player and the rich `#vod-timeline` below it both go through
  `bindScrubbing` + `seekFromPointer`, which take the element to measure
  against. The in-player bar carries no marker ticks; the timeline keeps the
  metric graph, marker glyphs and ruler. The timeline is *outside*
  `.player-wrap`, so it is unavailable in fullscreen. There, marker
  navigation is the `[` / `]` / `d` / `D` hotkeys, which are bound at the
  document level and keep working.
- **Speed and audio-track pickers sit behind the gear button**, in a popover
  that closes on outside pointerdown, on Escape, on `fullscreenchange` and in
  `closeReview`. Escape is guarded on the menu actually being open, so it
  never shadows the user agent's own Escape-exits-fullscreen. Volume is an
  icon that expands into a slider on hover or focus, with an `.open` class
  held for the duration of a drag so it cannot collapse mid-drag.
- The fullscreen button's state is synced from a `fullscreenchange` listener
  rather than from the click handler, since Escape and the OS can both leave
  fullscreen without going through the app.
- Markers closer together than the timeline can resolve (common around a
  teamfight) collapse into one cluster glyph; `MARKER_PRIORITY` decides which
  icon the cluster shows.
- **Audio stems.** Track 0 is the combined mix and plays from the `<video>`
  itself, so most recordings need nothing here and the picker stays hidden
  (fewer than two tracks, or an unknown layout). Selecting any other track
  calls `extract_audio_track`, then plays the returned sidecar through a
  hidden `<audio>` synced against the muted video, since WebView2 offers no way
  to switch tracks within one element
  ([DEVELOPMENT.md §2.5](../DEVELOPMENT.md#25-multi-track-audio)).
- Because of that, **volume and mute are held as state, not read off the video
  element** (`userVolume` / `userMuted` → `applyAudioOutput`). The video is
  muted whenever a stem is playing, and controls that read `video.muted` would
  render a muted player over audible sound. The `volumechange` listener was
  removed for the same reason: it would re-enter on the programmatic mute.

- **Playback stops while the window is hidden.** An open VOD otherwise keeps
  decoding video and playing its stem `<audio>` behind a minimised window,
  which is the largest thing the app can burn while it is out of the way.
  A `visibilitychange` listener pauses it and resumes only what it paused
  (`pausedByHide`), so a video the user had already paused stays paused.
  Pausing cascades through the existing `play`/`pause` handlers, so the rAF
  playhead loop stops with it, and `resumeStem` hard-resyncs the stem on the
  way back, so it cannot return drifted.

## Escaping, and why there is nothing left to call

`reconcile` imports any video file the user drops into the recordings folder,
so a displayed recording name is **not necessarily ours**, and `vodTitle`
falls back to a filename.

v1 answered that with two functions. `escapeHtml` was for text nodes and did
not handle quotes; `escapeAttr` was the one for attribute values; and every
interpolation site had to pick the right one. Using the wrong one was an
injection bug with a plausible trigger, and getting it right forty times in a
row was the only thing standing between a filename and the DOM.

**Both are gone, and so is the choice.** Svelte's default interpolation escapes
what it renders, `dom.ts` went with the vanilla shell in WS4.6, and `src/dev/ui.ts`
went with the portal rewrite in #72. There is no escape helper left to reach
for, which is the point: the way to render an untrusted string is to
interpolate it.

Three tests hold the line, and they answer different questions:

| Test | Claim |
|---|---|
| `Row.test.ts` | the most exposed component renders a hostile title as text |
| `UpdateNotes.test.ts` | release notes, fetched over HTTPS but **not covered by the update signature**, render as a parsed structure rather than markup |
| `markup-guard.test.ts` | no component anywhere contains `{@html`, and nothing in `src/` assigns `innerHTML`, `outerHTML`, `insertAdjacentHTML` or calls `document.write` |

The first two are about the components most likely to be handed something
nasty. The third is the structural one, and it is the half a per-component test
can never cover: it says no *future* component can reopen the hole. It checks
that it is reading a real tree before it checks anything else, because a guard
whose glob matches nothing passes forever, and it carries its own proof that
the matcher bites.

If a case for `{@html}` ever genuinely arrives, it needs an entry in that
test's `ALLOWED` list with a reason, a sanitiser, and a test of its own.
The list is empty today. Adding to it should be a deliberate, reviewed act,
which is exactly what failing the guard makes it.
