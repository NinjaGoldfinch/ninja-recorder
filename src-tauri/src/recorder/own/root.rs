//! Which process tree a process-loopback capture is rooted at, with no
//! Windows in it.
//!
//! Process loopback captures a **process tree**: the target and every
//! descendant (`PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`). Getting
//! the root wrong does not fail; it records silence, or someone else's
//! audio. So the choice is made here, from a snapshot of the process table
//! handed in by `own/win/process.rs` (Toolhelp, plus each process's creation
//! time), and every case is a unit test.
//!
//! Ported from `spikes/p0c-audio`, whose report settled the question on the
//! box (DEVELOPMENT.md §16): the root is `League of Legends.exe` itself,
//! found both by name and as the owner of the game window, and its ancestors
//! are `LeagueClient.exe` → `RiotClientServices.exe`, which the capture must
//! not include. Every Discord process sat outside that tree.
//!
//! Two questions, for two kinds of source:
//!
//! - [`game_root`]: the game process. The game window's owner is preferred,
//!   because it is the process whose window is being recorded, and it is
//!   cross-checked against the name so a window of the same class owned by
//!   something else is not trusted.
//! - [`application_root`]: the top of an application's own tree, for a
//!   multi-process app such as Discord, whose audio plays in a child of the
//!   process the user started (#238).

/// The game's executable, as Toolhelp names it.
pub const GAME_EXE: &str = "League of Legends.exe";

/// How deep an ancestor walk goes. A snapshot is not atomic, and a reused
/// PID can make the recorded parent links form a cycle.
const MAX_DEPTH: usize = 64;

/// One process, as the snapshot saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    /// The parent PID recorded when this process was created. Only a number:
    /// the parent may have exited, and its PID been reused since.
    pub ppid: u32,
    pub exe: String,
    /// Creation time as a FILETIME tick count, where the process could be
    /// opened for it. It is what tells a live parent from a reused PID.
    pub created: Option<u64>,
}

/// The root a capture should target, and how it was chosen, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub pid: u32,
    pub how: String,
}

fn find(procs: &[Proc], pid: u32) -> Option<&Proc> {
    procs.iter().find(|p| p.pid == pid)
}

fn named<'a>(procs: &'a [Proc], exe: &str) -> Vec<&'a Proc> {
    procs.iter().filter(|p| p.exe.eq_ignore_ascii_case(exe)).collect()
}

/// The live parent of `child`, or `None` if its recorded parent has exited.
///
/// Windows reuses PIDs, so a parent that exited leaves a number that may now
/// belong to something unrelated. A real parent was created before its
/// child, so a "parent" created later is a reused number, not a parent. When
/// either creation time is unknown the link is taken at face value, as the
/// spike did.
pub fn parent_of<'a>(procs: &'a [Proc], child: &Proc) -> Option<&'a Proc> {
    if child.ppid == 0 || child.ppid == child.pid {
        return None;
    }
    let parent = find(procs, child.ppid)?;
    match (parent.created, child.created) {
        (Some(p), Some(c)) if p > c => None,
        _ => Some(parent),
    }
}

/// `of`'s live ancestors, nearest first.
pub fn ancestors<'a>(procs: &'a [Proc], of: &'a Proc) -> Vec<&'a Proc> {
    let mut chain: Vec<&Proc> = Vec::new();
    let mut current = of;
    while chain.len() < MAX_DEPTH {
        match parent_of(procs, current) {
            // A cycle: stop rather than walk it again.
            Some(parent) if parent.pid == of.pid || chain.iter().any(|a| a.pid == parent.pid) => {
                break;
            }
            Some(parent) => {
                chain.push(parent);
                current = parent;
            }
            None => break,
        }
    }
    chain
}

/// Whether `candidate` is `root` or one of its descendants: what include
/// mode captures.
pub fn in_tree(procs: &[Proc], root: u32, candidate: &Proc) -> bool {
    candidate.pid == root || ancestors(procs, candidate).iter().any(|p| p.pid == root)
}

/// The game process, whose tree carries the game's audio.
///
/// `window_owner` is the PID that owns the game window (`RiotWindowClass`),
/// from `GetWindowThreadProcessId`, if there is a window.
///
/// 1. The window's owner, when it is named [`GAME_EXE`]: the process whose
///    window is being recorded.
/// 2. Otherwise the one process named [`GAME_EXE`]; the log says the window
///    disagreed.
/// 3. With several and no owner to choose between them, the newest: a game
///    left over from an earlier match is older than the one being recorded.
///    If their creation times are unknown there is no way to choose, and
///    this refuses rather than guess.
pub fn game_root(procs: &[Proc], window_owner: Option<u32>) -> Result<Root, String> {
    let games = named(procs, GAME_EXE);
    let owner = window_owner.and_then(|pid| find(procs, pid));

    if let Some(owner) = owner
        && owner.exe.eq_ignore_ascii_case(GAME_EXE)
    {
        let how = if games.len() > 1 {
            format!("the game window's owner, one of {} {GAME_EXE} processes", games.len())
        } else {
            format!("the game window's owner, named {GAME_EXE}")
        };
        return Ok(Root { pid: owner.pid, how });
    }

    let disagreement = match (window_owner, owner) {
        (None, _) => "no game window owner".to_string(),
        (Some(pid), None) => format!("the game window's owner, PID {pid}, is not in the snapshot"),
        (Some(pid), Some(p)) => {
            format!("the game window is owned by PID {pid} ({}), not {GAME_EXE}", p.exe)
        }
    };

    match games.as_slice() {
        [] => Err(format!("no {GAME_EXE} process is running ({disagreement})")),
        [only] => Ok(Root { pid: only.pid, how: format!("by name; {disagreement}") }),
        many => {
            let pids = many.iter().map(|p| p.pid.to_string()).collect::<Vec<_>>().join(", ");
            if many.iter().any(|p| p.created.is_none()) {
                return Err(format!(
                    "{} {GAME_EXE} processes ({pids}) and no way to choose: {disagreement}, and \
                     not every creation time could be read",
                    many.len()
                ));
            }
            let newest = many.iter().max_by_key(|p| p.created).expect("not empty");
            Ok(Root {
                pid: newest.pid,
                how: format!(
                    "the newest of {} {GAME_EXE} processes ({pids}); {disagreement}",
                    many.len()
                ),
            })
        }
    }
}

/// The top of `exe`'s own tree: the process named `exe` whose live parent is
/// not also named `exe`. Include mode on it captures every helper process
/// the application spawned under itself, which is where a multi-process app
/// such as Discord plays its audio.
///
/// Two separate tops (two instances, or a helper whose parent exited) are
/// resolved to the oldest, which is the one the user started; the log says
/// how many there were.
pub fn application_root(procs: &[Proc], exe: &str) -> Result<Root, String> {
    let all = named(procs, exe);
    let tops: Vec<&Proc> = all
        .iter()
        .copied()
        .filter(|p| parent_of(procs, p).is_none_or(|parent| !parent.exe.eq_ignore_ascii_case(exe)))
        .collect();
    match tops.as_slice() {
        [] if all.is_empty() => Err(format!("no {exe} process is running")),
        // Every one has a parent of the same name: a cycle in the recorded
        // links. Fall back to the oldest process of that name.
        [] => {
            let oldest = oldest(&all);
            Ok(Root {
                pid: oldest.pid,
                how: format!("the oldest of {} {exe} processes (no top found)", all.len()),
            })
        }
        [only] => Ok(Root {
            pid: only.pid,
            how: format!("the top of {exe}'s tree ({} processes)", all.len()),
        }),
        many => {
            let top = oldest(many);
            Ok(Root {
                pid: top.pid,
                how: format!(
                    "the oldest of {} separate {exe} trees ({} processes)",
                    many.len(),
                    all.len()
                ),
            })
        }
    }
}

/// The oldest process by creation time; one with an unknown time sorts
/// last, then the lowest PID wins so the answer is stable.
fn oldest<'a>(procs: &[&'a Proc]) -> &'a Proc {
    procs
        .iter()
        .copied()
        .min_by_key(|p| (p.created.is_none(), p.created, p.pid))
        .expect("not empty")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, exe: &str, created: u64) -> Proc {
        Proc { pid, ppid, exe: exe.to_string(), created: Some(created) }
    }

    /// The tree §16 measured on the box: the game under the client under the
    /// Riot client, Discord beside it in its own multi-process tree.
    fn measured() -> Vec<Proc> {
        vec![
            p(4, 0, "System", 1),
            p(1000, 900, "explorer.exe", 10),
            p(2000, 1000, "RiotClientServices.exe", 20),
            p(3000, 2000, "LeagueClient.exe", 30),
            p(3100, 3000, "LeagueClientUx.exe", 31),
            p(3200, 3100, "LeagueClientUxRender.exe", 32),
            p(4000, 3000, "League of Legends.exe", 40),
            p(5000, 1000, "Discord.exe", 15),
            p(5100, 5000, "Discord.exe", 16),
            p(5200, 5000, "Discord.exe", 17),
            p(5300, 5000, "Discord.exe", 18),
            p(6000, 1000, "vgtray.exe", 12),
        ]
    }

    #[test]
    fn the_measured_tree_roots_at_the_game_by_its_window() {
        let procs = measured();
        let root = game_root(&procs, Some(4000)).unwrap();
        assert_eq!(root.pid, 4000);
        assert!(root.how.contains("window's owner"), "{}", root.how);

        // Its ancestors are the client and the Riot client, which include
        // mode does not capture.
        let game = find(&procs, 4000).unwrap();
        let chain: Vec<&str> = ancestors(&procs, game).iter().map(|p| p.exe.as_str()).collect();
        assert_eq!(chain, ["LeagueClient.exe", "RiotClientServices.exe", "explorer.exe"]);

        // Nothing but the game is in its tree: no client, no Discord.
        let captured: Vec<u32> =
            procs.iter().filter(|c| in_tree(&procs, 4000, c)).map(|c| c.pid).collect();
        assert_eq!(captured, [4000]);
    }

    #[test]
    fn the_game_is_found_by_name_when_there_is_no_window() {
        let root = game_root(&measured(), None).unwrap();
        assert_eq!(root.pid, 4000);
        assert!(root.how.contains("by name") && root.how.contains("no game window"), "{}", root.how);
    }

    #[test]
    fn a_window_owner_that_disagrees_with_the_name_is_not_trusted() {
        // A window of the game's class owned by the client: the name wins,
        // and the log says the two disagreed.
        let root = game_root(&measured(), Some(3000)).unwrap();
        assert_eq!(root.pid, 4000);
        assert!(root.how.contains("LeagueClient.exe"), "{}", root.how);

        // An owner that has already gone from the snapshot.
        let root = game_root(&measured(), Some(9999)).unwrap();
        assert_eq!(root.pid, 4000);
        assert!(root.how.contains("9999"), "{}", root.how);
    }

    #[test]
    fn no_game_process_is_an_error_naming_the_window() {
        let procs: Vec<Proc> =
            measured().into_iter().filter(|p| p.exe != GAME_EXE).collect();
        let err = game_root(&procs, Some(3000)).unwrap_err();
        assert!(err.contains(GAME_EXE) && err.contains("LeagueClient.exe"), "{err}");
    }

    #[test]
    fn two_game_processes_are_told_apart_by_the_window_then_by_age() {
        let mut procs = measured();
        // A game left over from an earlier match, older than this one.
        procs.push(p(3900, 3000, "League of Legends.exe", 35));

        let root = game_root(&procs, Some(3900)).unwrap();
        assert_eq!(root.pid, 3900, "the window's owner wins, even if older");
        assert!(root.how.contains("one of 2"), "{}", root.how);

        let root = game_root(&procs, None).unwrap();
        assert_eq!(root.pid, 4000, "with no window, the newest");
        assert!(root.how.contains("newest of 2"), "{}", root.how);

        // Without creation times there is nothing to choose by.
        for proc in procs.iter_mut().filter(|p| p.exe == GAME_EXE) {
            proc.created = None;
        }
        let err = game_root(&procs, None).unwrap_err();
        assert!(err.contains("3900") && err.contains("4000"), "{err}");
    }

    #[test]
    fn a_reused_parent_pid_is_not_a_parent() {
        let mut procs = measured();
        // The client exited and its PID was taken by something newer than
        // the game: the game has no live ancestors, and the newcomer is not
        // above it.
        procs.retain(|p| p.pid != 3000);
        procs.push(p(3000, 1000, "notepad.exe", 50));
        let game = find(&procs, 4000).unwrap();
        assert!(parent_of(&procs, game).is_none());
        assert!(ancestors(&procs, game).is_empty());
        assert!(!in_tree(&procs, 3000, game), "a reused PID does not own the game");
        assert_eq!(game_root(&procs, Some(4000)).unwrap().pid, 4000);

        // The same link with no creation time on one side is taken at face
        // value, as the spike did.
        let procs: Vec<Proc> = procs
            .into_iter()
            .map(|mut q| {
                if q.pid == 3000 {
                    q.created = None;
                }
                q
            })
            .collect();
        let game = find(&procs, 4000).unwrap();
        assert_eq!(parent_of(&procs, game).map(|q| q.pid), Some(3000));
    }

    #[test]
    fn a_cycle_in_the_recorded_links_ends_the_walk() {
        let procs = vec![
            Proc { pid: 1, ppid: 2, exe: "a.exe".into(), created: None },
            Proc { pid: 2, ppid: 1, exe: "b.exe".into(), created: None },
        ];
        let a = find(&procs, 1).unwrap();
        assert_eq!(ancestors(&procs, a).len(), 1);
        assert!(!in_tree(&procs, 3, a));
    }

    #[test]
    fn discord_roots_at_the_top_of_its_own_tree() {
        let procs = measured();
        let root = application_root(&procs, "discord.exe").unwrap();
        assert_eq!(root.pid, 5000);
        assert!(root.how.contains("4 processes"), "{}", root.how);
        // Every Discord helper is inside that tree, and the game is not.
        for c in procs.iter().filter(|c| c.exe == "Discord.exe") {
            assert!(in_tree(&procs, 5000, c), "{}", c.pid);
        }
        assert!(!in_tree(&procs, 5000, find(&procs, 4000).unwrap()));
        // And no Discord process is inside the game's.
        assert!(procs.iter().filter(|c| c.exe == "Discord.exe").all(|c| !in_tree(&procs, 4000, c)));
    }

    #[test]
    fn discord_whose_launcher_exited_and_whose_pid_was_reused_still_has_one_top() {
        let mut procs = measured();
        // Update.exe launched Discord and exited; its PID now belongs to a
        // newer Discord helper. That helper is not Discord's parent.
        for q in procs.iter_mut().filter(|q| q.pid == 5000) {
            q.ppid = 7000;
        }
        procs.push(p(7000, 5000, "Discord.exe", 60));
        let root = application_root(&procs, "Discord.exe").unwrap();
        assert_eq!(root.pid, 5000);
    }

    #[test]
    fn two_discord_trees_resolve_to_the_oldest() {
        let mut procs = measured();
        procs.push(p(8000, 1000, "Discord.exe", 70));
        procs.push(p(8100, 8000, "Discord.exe", 71));
        let root = application_root(&procs, "Discord.exe").unwrap();
        assert_eq!(root.pid, 5000);
        assert!(root.how.contains("2 separate"), "{}", root.how);
    }

    #[test]
    fn an_application_that_is_not_running_is_an_error() {
        assert!(application_root(&measured(), "Spotify.exe").is_err());
    }

    #[test]
    fn an_application_whose_every_process_has_a_same_named_parent_falls_back_to_the_oldest() {
        let procs = vec![
            Proc { pid: 1, ppid: 2, exe: "x.exe".into(), created: None },
            Proc { pid: 2, ppid: 1, exe: "x.exe".into(), created: Some(5) },
        ];
        assert_eq!(application_root(&procs, "x.exe").unwrap().pid, 2);
    }
}
