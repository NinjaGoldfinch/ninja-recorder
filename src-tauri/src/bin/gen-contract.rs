//! Writes the generated TypeScript client, or checks that it is current.
//!
//! ```text
//! cargo run --features contract-gen --bin gen-contract              # write the files
//! cargo run --features contract-gen --bin gen-contract -- --check   # fail if they are stale
//! ```
//!
//! The emitter itself is `contract::r#gen`; this is only argv, file I/O and an
//! exit code, so that the part worth testing has no I/O in it.
//!
//! `--check` is the CI gate. It is what replaces `every_command_round_trips`
//! and the dev portal's drift banner, because a hand-written list that
//! disagrees with the table now fails the build rather than being caught by a
//! test that happens to exercise the same names.

use std::process::ExitCode;

use ninja_recorder_lib::gen_contract;

fn main() -> ExitCode {
    let check = std::env::args().skip(1).any(|a| a == "--check");
    let unknown: Vec<String> =
        std::env::args().skip(1).filter(|a| a != "--check").collect();
    if !unknown.is_empty() {
        eprintln!("gen-contract: unrecognised argument(s): {}", unknown.join(", "));
        eprintln!("usage: gen-contract [--check]");
        // 2 rather than 1, so "you called it wrong" is distinguishable from
        // "the contract is stale" by a script.
        return ExitCode::from(2);
    }

    let root = gen_contract::repo_root();
    let files = gen_contract::emit();

    if check {
        let mut stale = Vec::new();
        for file in &files {
            let path = root.join(&file.path);
            match std::fs::read_to_string(&path) {
                Ok(on_disk) if on_disk == file.contents => {}
                Ok(_) => stale.push(format!("{} differs", file.path.display())),
                Err(e) => stale.push(format!("{} could not be read: {e}", file.path.display())),
            }
        }
        if stale.is_empty() {
            println!("gen-contract: the committed contract is current ({} files)", files.len());
            return ExitCode::SUCCESS;
        }
        eprintln!("gen-contract: the committed TypeScript does not match the Rust declaration.");
        for line in &stale {
            eprintln!("  {line}");
        }
        eprintln!();
        eprintln!("Run `cargo run --features contract-gen --bin gen-contract` from src-tauri/ and commit the result.");
        return ExitCode::FAILURE;
    }

    for file in &files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("gen-contract: could not create {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = std::fs::write(&path, &file.contents) {
            eprintln!("gen-contract: could not write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("gen-contract: wrote {}", file.path.display());
    }
    ExitCode::SUCCESS
}
