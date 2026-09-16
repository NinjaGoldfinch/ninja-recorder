/**
 * The daemon transport. WS3 task 3.4.
 *
 * `invoke.ts` ran every command in the process hosting this webview.
 * This runs them in the daemon, by handing the name and arguments to
 * `rpc_call`, which forwards them over the pipe. The wire shape, the argument
 * spelling and the error strings are the ones `core::dispatch` already
 * produced, so a view written against `call` or the generated client does not
 * know the difference.
 *
 * What is genuinely new is the other direction. The daemon pushes: a snapshot
 * on every handshake, then a stream of events. A view no longer asks "what is
 * the state" on a timer; it is told, and `subscribe` below is how it hears.
 *
 * ## Why the shell commands still go direct
 *
 * `open_recordings_folder` and the portal's window and file-manager commands
 * belong to the process with a window. Sending them to the daemon would open an
 * Explorer window from a background process, which lands behind the foreground
 * app. The list is the same one `invoke.ts` carries, and for the same reason.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { Event } from "../contract/events";
import type { Snapshot } from "../contract/types";
import type { Transport } from "./index";

/**
 * Commands that run in *this* process rather than the daemon.
 *
 * Deliberately the same set `invoke.ts` sends direct, minus the `dev_*` ones,
 * which the portal's own layer (`src/dev/ipc.ts`) routes from the generated
 * catalogue.
 */
const DIRECT_COMMANDS = new Set([
  "open_recordings_folder",
  "dev_open_portal",
  "dev_registered_commands",
]);

/** How the connection to the daemon is doing. */
export type DaemonHealth =
  | { state: "connected" }
  | { state: "reconnecting" }
  /** Terminal: this UI and that daemon are from different builds. */
  | { state: "skewed"; ours: number; theirs: number };

export const pipeTransport: Transport = {
  invoke(command, args) {
    if (DIRECT_COMMANDS.has(command)) return invoke(command, args);
    return invoke("rpc_call", { command, args: args ?? {} });
  },
};

/**
 * The snapshot the daemon last sent, or `null` if no handshake has completed.
 *
 * `null` is a real state on a cold start, not an error: the daemon is being
 * launched while this window is already painting. Render the empty state; the
 * snapshot callback fires a moment later with the real one, which is the same
 * path a reconnect takes.
 */
export function currentSnapshot(): Promise<Snapshot | null> {
  return invoke<Snapshot | null>("rpc_subscribe");
}

/** What the connection is doing right now. */
export function daemonHealth(): Promise<DaemonHealth> {
  return invoke<DaemonHealth>("rpc_health");
}

/**
 * Everything the daemon pushes, in one subscription.
 *
 * `onSnapshot` fires on every handshake, which means on connect *and* on every
 * reconnect. Each one replaces the world rather than updating it: a client that
 * folded a snapshot in as though it were an event would merge a fresh state
 * into a stale one. That is what makes a window killed mid-game correct the
 * moment it comes back, and it is why the daemon sends one on `hello` rather
 * than replaying history.
 *
 * Returns a function that stops listening.
 */
export function subscribe(handlers: {
  onSnapshot?: (snapshot: Snapshot) => void;
  onEvent?: (event: Event) => void;
  onHealth?: (health: DaemonHealth) => void;
}): () => void {
  const unlisten: Array<Promise<() => void>> = [];

  if (handlers.onSnapshot) {
    const onSnapshot = handlers.onSnapshot;
    unlisten.push(listen<Snapshot>("snapshot", (e) => onSnapshot(e.payload)));
  }
  if (handlers.onEvent) {
    const onEvent = handlers.onEvent;
    unlisten.push(listen<Event>("event", (e) => onEvent(e.payload)));
  }
  if (handlers.onHealth) {
    const onHealth = handlers.onHealth;
    unlisten.push(listen<DaemonHealth>("daemon-health", (e) => onHealth(e.payload)));
  }

  return () => {
    // Each `listen` resolves to its own unlisten function. Awaiting them here
    // would make teardown async, which a caller unmounting a view cannot do, so
    // they are chained instead.
    for (const pending of unlisten) {
      void pending.then((off) => off());
    }
  };
}
