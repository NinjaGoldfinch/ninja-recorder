//! The one list of `dev_*` commands. WS2 task 2.7.
//!
//! ## Why this exists
//!
//! `generate_handler!` takes a literal path list: it cannot host a `#[cfg]`
//! attribute, and it cannot host a macro expansion inside its brackets. So the
//! registration list in `lib.rs` was hand-written, and `src/dev/registry.ts`
//! was a second hand-written copy of the same names on the TypeScript side.
//! Two hand-maintained lists is the condition the portal's drift banner existed
//! to make visible.
//!
//! This is the callback-macro shape `contract::types`' boundary list already
//! uses: the list lives here once, and a caller passes a macro that receives
//! it. `lib.rs` hands it one that expands to `generate_handler!`, so the
//! registration and the manifest come from the same tokens and cannot
//! disagree. Issue #74 is the decision that put the `dev_*` commands in the
//! declaration at all.
//!
//! ## What it deliberately does not change
//!
//! Joining the *declaration* is not joining the `rpc` *dispatch*. These stay
//! individually registered behind the `devtools` feature and the portal keeps
//! invoking them through `src/dev/ipc.ts`; moving them onto the pipe is WS3.7.
//! `dev_registered_commands` in particular must stay directly registered,
//! because `devportal.ts` decides whether the portal exists by watching that
//! call reject in a shipped build.

/// Hands `$m` every `dev_*` command name, in registration order.
///
/// Registration order rather than alphabetical: this is the list that becomes
/// `generate_handler!`, and keeping it in the order the modules are laid out
/// makes a missing command visible next to its neighbours.
#[macro_export]
macro_rules! dev_command_list {
    ($m:ident) => {
        $m!(
        dev_open_portal,
        dev_env_info,
        dev_health,
        dev_registered_commands,
        dev_open_data_dir,
        dev_log_files,
        dev_read_log,
        dev_schema,
        dev_table_page,
        dev_sql_query,
        dev_insert_row,
        dev_update_row,
        dev_delete_row,
        dev_reset_db,
        dev_seed_library,
        dev_clear_seeded,
        dev_retention_preview,
        dev_dispatch_state_event,
        dev_inject_snapshot,
        dev_session_snapshot,
        dev_replay_start,
        dev_replay_stop,
        dev_replay_status,
        dev_lcu_get,
        dev_champion_name,
        dev_fetch_match_summary,
        dev_patch_match_summary,
        dev_live_client_probe,
        dev_fixtures_state,
        dev_shape_report,
        dev_recording_report,
        dev_recording_vs_lcu,
        dev_backfill_recording,
        dev_reveal_recording,
        dev_open_fixture,
        dev_ranked_stats,
        dev_lobby_rank,
        dev_lp_delta,
        dev_event_capture_start,
        dev_event_capture_stop,
        dev_event_capture_status,
        dev_event_uris,
        dev_fixture_read,
        dev_fixture_write,
        dev_set_fixture_recording,
        dev_trim_lead_in,
        )
    };
}

/// Every `dev_*` command name, for the portal and for the drift-free manifest.
///
/// Built from the same tokens the handler is, so a command registered without
/// being named here does not compile and one named here without being
/// registered does not either.
// Its consumer is the generated portal spec, which lands in the same task.
// Clippy runs without `--all-targets`, so until then this is genuinely dead
// code in every build and `-D warnings` would fail on it: the same treatment,
// and the same reason, as `core::dispatch::contract_manifest`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn dev_command_names() -> &'static [&'static str] {
    macro_rules! names {
        ($($n:ident),* $(,)?) => { &[ $( stringify!($n), )* ] };
    }
    dev_command_list!(names)
}

#[cfg(test)]
mod tests {
    /// The property the whole shape exists for: the list that becomes
    /// `generate_handler!` and the list the portal is told about are the same
    /// tokens, so they cannot disagree.
    ///
    /// This cannot catch a command that was never added to the list at all,
    /// because nothing can reflect over `generate_handler!`. What it does catch
    /// is the list being edited without the manifest following, which is the
    /// failure the deleted drift banner was watching for.
    #[test]
    fn every_name_is_a_dev_command_and_they_are_unique() {
        let names = super::dev_command_names();
        assert!(!names.is_empty());
        for n in names {
            assert!(n.starts_with("dev_"), "{n} is in the dev list but is not a dev_* command");
        }
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "a command is listed twice");
    }
}
