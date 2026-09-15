//! TypeScript emitter for the contract. WS2 task 2.5.
//!
//! Walks the two declarations, `core::dispatch`'s table and
//! `contract::events`' table, and writes `src/lib/contract/{types,client,
//! events}.ts`. Run as `cargo run --bin gen-contract`; `--check` re-emits into
//! a buffer and fails if it differs from what is committed, which is the CI
//! gate.
//!
//! The output is committed rather than generated at build time, so a frontend
//! developer with no Rust toolchain can still work and so the diff of a
//! contract change is reviewable (implementation plan §4.1).
//!
//! ## Where the type names come from, and the map that is not here
//!
//! The obvious shape for this file is a Rust-path-to-TypeScript lookup:
//! `i64` becomes `number`, `Vec<T>` becomes `Array<T>`, and so on, parsed out
//! of the `stringify!`'d spellings the manifests carry. That is the one thing
//! this module must not do. It would be a third list, able to disagree with
//! the dispatch table and the type declarations, in the workstream whose whole
//! purpose is deleting lists that can disagree.
//!
//! Instead the macros resolve the names themselves, where they still have the
//! real types in hand: `dispatch_table!` emits `contract_manifest_ts`, and
//! every boundary type answers `ts_rs::TS::decl`. ts-rs already knows how each
//! one renders, generics included. So there is no mapping in this file, and a
//! type it cannot render is a compile error in the table rather than a wrong
//! string in the output.
//!
//! ## Why four files
//!
//! They have different reasons to change. `types.ts` changes when a struct
//! crossing the boundary changes shape; `client.ts` when a command is added or
//! its signature moves; `events.ts` when an event joins a topic. One file would
//! make every contract change look like it touched everything. `index.ts` is a
//! barrel, generated with the rest so the whole directory has one origin.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::contract::events::{Topic, event_manifest};
use crate::contract::types::{boundary_declarations, config};
use crate::core::dispatch::contract_manifest_ts;

/// Written at the top of every emitted file.
///
/// Says the two things a reader needs: do not edit this, and here is the
/// command that regenerates it. Deliberately not a timestamp or a version,
/// which would make every regeneration a diff even when nothing changed.
const HEADER: &str = "\
// GENERATED FILE. Do not edit by hand.
//
// Regenerate with:  cargo run --bin gen-contract   (from src-tauri/)
// CI runs the same binary with --check and fails if this file is stale.
//
// The declaration lives in Rust:
//   commands  src-tauri/src/core/dispatch.rs   (dispatch_table!)
//   events    src-tauri/src/contract/events.rs (contract_events!)
//   types     src-tauri/src/contract/types.rs  (the boundary list)
";

/// One emitted file: where it goes, and what is in it.
pub struct Emitted {
    /// Relative to the repository root, so a failure message can name a path a
    /// reader can actually open.
    pub path: PathBuf,
    pub contents: String,
}

/// Emit all three files into memory.
///
/// Writing is a separate step so `--check` can compare without touching the
/// working tree, which matters because CI runs it on a clean checkout and a
/// generator that wrote before comparing would always pass.
pub fn emit() -> Vec<Emitted> {
    let cfg = config();
    vec![
        Emitted { path: PathBuf::from("src/lib/contract/types.ts"), contents: types_ts(&cfg) },
        Emitted { path: PathBuf::from("src/lib/contract/events.ts"), contents: events_ts(&cfg) },
        Emitted { path: PathBuf::from("src/lib/contract/client.ts"), contents: client_ts(&cfg) },
        Emitted { path: PathBuf::from("src/lib/contract/index.ts"), contents: index_ts() },
        Emitted { path: PathBuf::from("src/dev/commands.generated.ts"), contents: portal_ts() },
    ]
}

/// The dev portal's command catalogue. WS2.7.
///
/// This replaces `src/dev/registry.ts`, which was a hand-written copy of the
/// command surface in TypeScript, and the drift banner that watched it for
/// disagreement. Both existed only because there were two lists; there is now
/// one, in Rust, and this is its TypeScript face.
///
/// It lands in `src/dev/` rather than `src/lib/contract/` because the dev
/// portal is compiled out of shipped builds and nothing in the app proper may
/// import it. `--check` covers it exactly as it covers the others.
fn portal_ts() -> String {
    use crate::core::dispatch::command_descriptions;
    use crate::contract::portal::{dev_command_manifest, production_form_manifest};

    let descriptions: std::collections::HashMap<&str, &str> =
        command_descriptions().iter().copied().collect();

    let mut out = String::from(HEADER);
    out.push('\n');
    out.push_str(
        r#"export interface PortalArg {
  name: string;
  kind: "string" | "number" | "boolean" | "json";
  /** Pre-filled into the form. Empty means no default. */
  default: string;
  /** Rendered under the input. Empty when nobody has written one. */
  help: string;
  optional: boolean;
}

export interface PortalCommand {
  name: string;
  group: string;
  /** `dev_*` commands are compiled out of shipped builds. */
  dev: boolean;
  /** Writes, deletes, or otherwise cannot simply be re-run. */
  danger: boolean;
  description: string;
  args: PortalArg[];
}

"#,
    );

    let _ = writeln!(out, "export const COMMANDS: PortalCommand[] = [");
    for (spec, is_dev) in production_form_manifest()
        .iter()
        .map(|s| (s, false))
        .chain(dev_command_manifest().iter().map(|s| (s, true)))
    {
        // A production command's help text lives on its `dispatch_table!` row;
        // a dev command's lives on its row here. One home each, neither copied.
        // `/// text` captures `" text"`, and a multi-line comment concatenates
        // with those spaces still in, so the prose is normalised here rather
        // than every row being written to work around the macro.
        let raw = if is_dev {
            spec.description
        } else {
            descriptions.get(spec.name).copied().unwrap_or("")
        };
        let description: String =
            raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let _ = writeln!(out, "  {{");
        let _ = writeln!(out, "    name: {},", quote(spec.name));
        let _ = writeln!(out, "    group: {},", quote(spec.group));
        let _ = writeln!(out, "    dev: {is_dev},");
        let _ = writeln!(out, "    danger: {},", spec.danger);
        let _ = writeln!(out, "    description: {},", quote(&description));
        if spec.args.is_empty() {
            let _ = writeln!(out, "    args: [],");
        } else {
            let _ = writeln!(out, "    args: [");
            for a in spec.args {
                let _ = writeln!(
                    out,
                    "      {{ name: {}, kind: {}, default: {}, help: {}, optional: {} }},",
                    quote(a.name),
                    quote(a.kind),
                    quote(a.default),
                    quote(a.help),
                    a.optional
                );
            }
            let _ = writeln!(out, "    ],");
        }
        let _ = writeln!(out, "  }},");
    }
    let _ = writeln!(out, "];");
    out
}

/// A TypeScript double-quoted string literal.
///
/// Hand-rolled rather than via `serde_json`, which would be a dependency taken
/// for one function and would still need the same escapes checked.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The barrel.
///
/// Generated like the rest so the whole directory has one origin. A
/// hand-written file in here would be the one thing `--check` does not cover,
/// sitting next to three that it does.
fn index_ts() -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    out.push_str(
        "export * from \"./types\";\nexport * from \"./client\";\n\
         export { EVENTS_BY_TOPIC } from \"./events\";\n",
    );
    out
}

/// Every boundary type's declaration, in the order the list declares them.
///
/// Declaration order rather than dependency order: TypeScript `type` aliases
/// hoist, so a type may refer to one declared below it, and sorting by
/// dependency would reshuffle the file every time a field changed.
fn types_ts(cfg: &ts_rs::Config) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for (name, decl) in boundary_declarations(cfg) {
        // ts-rs renders `type Name = ...` without the export keyword.
        let _ = writeln!(out, "export {decl}\n");
        debug_assert!(!name.is_empty());
    }
    out
}

/// The event surface, re-exported and indexed by topic.
///
/// `Event` and `Topic` are declared in `types.ts` like every other boundary
/// type, and re-exported here rather than declared a second time. Two
/// declarations of the same union is exactly the drift this workstream exists
/// to stop, and TypeScript refuses the duplicate through the barrel anyway.
/// What this file adds is the part `types.ts` cannot express: which topic
/// carries which event.
fn events_ts(_cfg: &ts_rs::Config) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');

    out.push_str("import type { Event, Topic } from \"./types\";\n\n");
    out.push_str("export type { Event, Topic };\n\n");

    out.push_str(
        "/**\n * Which topic carries which event.\n *\n\
         * A client subscribes by topic, never by event name, which is what lets\n\
         * a variant be added to an existing topic without a client change.\n */\n",
    );
    let _ = writeln!(out, "export const EVENTS_BY_TOPIC = {{");
    for topic in [Topic::Recording, Topic::Lcu, Topic::Library, Topic::Update, Topic::Daemon] {
        let names: Vec<String> = event_manifest()
            .iter()
            .filter(|e| e.topic == topic)
            .map(|e| format!("\"{}\"", lower_camel(e.name)))
            .collect();
        let key = lower_camel(&format!("{topic:?}"));
        let _ = writeln!(out, "  {key}: [{}],", names.join(", "));
    }
    let _ = writeln!(out, "}} as const;");
    out
}

/// One method per command, with camelCase arguments and a typed return.
fn client_ts(cfg: &ts_rs::Config) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');

    let manifest = contract_manifest_ts(cfg);

    // Only the types this file actually mentions, so adding a boundary type
    // does not churn the client's import list.
    let mut used: Vec<String> = manifest
        .iter()
        .flat_map(|c| {
            std::iter::once(c.returns.clone()).chain(c.args.iter().map(|a| a.ts.clone()))
        })
        .flat_map(|t| named_types(&t))
        .collect();
    used.sort();
    used.dedup();
    if !used.is_empty() {
        let _ = writeln!(out, "import type {{ {} }} from \"./types\";\n", used.join(", "));
    }

    out.push_str(
        "/**\n * How a command reaches the backend. Supplied by the caller rather\n * than imported, so this file stays free of any transport: the Tauri\n * `invoke` today, a pipe frame under WS3, and a mock in tests.\n */\nexport type Invoke = (command: string, args: Record<string, unknown>) => Promise<unknown>;\n\n",
    );

    // The method name *is* the wire name, deliberately. Camel-casing it would
    // add a second mapping between the client and the dispatch table, which is
    // the class of thing this workstream exists to delete; argument names are
    // camelCased only because serde's `rename_all` already renamed them and the
    // client has to match what the backend will actually parse.
    out.push_str(
        "/** Every command, as one object. The key is the wire name. */\n\
         export function createClient(invoke: Invoke) {\n  return {\n",
    );
    for c in &manifest {
        let params: Vec<String> =
            c.args.iter().map(|a| format!("{}: {}", a.name, a.ts)).collect();
        let payload: Vec<String> = c.args.iter().map(|a| a.name.clone()).collect();
        let args_obj =
            if payload.is_empty() { "{}".to_string() } else { format!("{{ {} }}", payload.join(", ")) };
        let _ = writeln!(
            out,
            "    {}: ({}): Promise<{}> =>\n      invoke(\"{}\", {}) as Promise<{}>,",
            c.name,
            params.join(", "),
            c.returns,
            c.name,
            args_obj,
            c.returns
        );
    }
    out.push_str("  };\n}\n");
    out
}

/// The named types inside a rendered TypeScript type.
///
/// `Array<RecordingRow>` needs `RecordingRow` imported and `Array` not;
/// `{ [key in string]: string }` needs nothing. Rather than parse TypeScript,
/// this keeps only identifiers that are actually declared on the boundary,
/// which is the question being asked.
fn named_types(rendered: &str) -> Vec<String> {
    let cfg = config();
    let declared: Vec<String> =
        boundary_declarations(&cfg).into_iter().map(|(name, _)| name).collect();
    declared.into_iter().filter(|d| mentions_identifier(rendered, d)).collect()
}

/// Whether `haystack` uses `ident` as a whole word.
///
/// A substring test would match `Marker` inside `MarkerRow` and import a type
/// the file never names.
fn mentions_identifier(haystack: &str, ident: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `RecordingStarted` becomes `recordingStarted`, matching the `rename_all`
/// the event enum and the command names already use on the wire.
fn lower_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The repository root, from this crate's manifest directory.
///
/// `CARGO_MANIFEST_DIR` is `src-tauri/`, and every emitted path is relative to
/// its parent, so the binary works from any working directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri always has a parent")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_camel_leaves_an_already_camel_name_alone() {
        assert_eq!(lower_camel("recordingStarted"), "recordingStarted");
        assert_eq!(lower_camel("RecordingStarted"), "recordingStarted");
        assert_eq!(lower_camel(""), "");
    }

    /// The bug a substring test would have: `Marker` is a boundary type and so
    /// is `MarkerRow`, and importing the first because the second appeared
    /// would be a type error in the generated file.
    #[test]
    fn an_identifier_is_matched_whole_or_not_at_all() {
        assert!(mentions_identifier("Array<MarkerRow>", "MarkerRow"));
        assert!(!mentions_identifier("Array<MarkerRow>", "Marker"));
        assert!(mentions_identifier("Marker", "Marker"));
        assert!(!mentions_identifier("{ [key in string]: string }", "Marker"));
    }

    #[test]
    fn every_file_carries_the_do_not_edit_header() {
        for file in emit() {
            assert!(
                file.contents.starts_with("// GENERATED FILE."),
                "{} is missing the header",
                file.path.display()
            );
        }
    }

    /// The whole point of the task: a command in the table is a method on the
    /// client, with its arguments camelCased and its return type named.
    #[test]
    fn the_client_carries_every_command() {
        let cfg = config();
        let client = client_ts(&cfg);
        for c in contract_manifest_ts(&cfg) {
            assert!(
                client.contains(&format!("invoke(\"{}\"", c.name)),
                "{} is missing from the generated client",
                c.name
            );
        }
        // Spot-check the shape rather than the whole file, which the committed
        // output and `--check` already pin exactly.
        // The key is the wire name; only the *arguments* are camelCased, because
        // that is the only rename serde actually performs.
        assert!(
            client.contains("get_recording_markers: (recordingId: number): Promise<Array<MarkerRow>>"),
            "{client}"
        );
        assert!(client.contains("start_recording: (): Promise<null>"), "{client}");
    }

    #[test]
    fn the_event_union_and_its_topics_are_emitted() {
        let cfg = config();
        let events = events_ts(&cfg);
        // Declared once, in `types.ts`, and re-exported here rather than
        // declared a second time.
        assert!(events.contains("export type { Event, Topic };"), "{events}");
        assert!(events.contains("import type { Event, Topic } from \"./types\";"), "{events}");
        assert!(events.contains("EVENTS_BY_TOPIC"));
        // Every declared event has to appear under exactly one topic.
        for spec in event_manifest() {
            let wire = lower_camel(spec.name);
            assert!(
                events.contains(&format!("\"{wire}\"")),
                "{wire} is in no topic list"
            );
        }
    }

    /// `types.ts` is what `client.ts` imports from, so a type the client names
    /// and the types file does not declare is a broken build.
    #[test]
    fn everything_the_client_imports_is_declared() {
        let cfg = config();
        let declared: Vec<String> =
            boundary_declarations(&cfg).into_iter().map(|(n, _)| n).collect();
        for c in contract_manifest_ts(&cfg) {
            for t in std::iter::once(c.returns.clone()).chain(c.args.iter().map(|a| a.ts.clone())) {
                for name in named_types(&t) {
                    assert!(declared.contains(&name), "{name} is used but not declared");
                }
            }
        }
    }
}
