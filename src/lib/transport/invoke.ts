/**
 * The Tauri transport. WS2 task 2.6.
 *
 * What v1 does today, and what keeps working right through WS2: every
 * production command goes through the single `rpc` passthrough, which
 * `core::dispatch` routes by name (DEVELOPMENT.md §12). WS3 replaces this with
 * `pipe.ts` without any caller changing.
 *
 * Lifted from `bridge.ts` unchanged, including the direct-command list, which
 * is behaviour rather than plumbing.
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

import type { Transport } from "./index";

/**
 * Whether the page is inside the Tauri webview at all.
 *
 * Read once: it cannot change for the life of the document, and the plain
 * `vite` dev server needs to be able to answer "no" before anything else runs.
 */
export const IN_TAURI = "__TAURI_INTERNALS__" in window;

/**
 * Commands that are *not* in the Rust dispatch table and so must be invoked
 * directly. Everything else goes through the `rpc` passthrough, which is what
 * lets the backend register one command instead of twenty-three and generate
 * its name list instead of hand-writing it (DEVELOPMENT.md §12).
 *
 * Two reasons a command is on this list:
 *   - it drives the desktop shell, so it belongs to the UI process and not
 *     the recorder: `open_recordings_folder`, `dev_open_portal`;
 *   - it is a `dev_*` command, which stays individually registered behind the
 *     `devtools` Cargo feature. `dev_registered_commands` in particular *must*
 *     stay direct: `hasDevCommands` detects whether the portal exists by
 *     seeing that call reject in a shipped build, and routing it through `rpc`
 *     would
 *     make it reject with "unknown command" in *every* build, permanently
 *     hiding the button.
 *
 * The portal's own panels use a separate invoke layer (`src/dev/ipc.ts`) and
 * are unaffected.
 */
const DIRECT_COMMANDS = new Set([
  "open_recordings_folder",
  "dev_open_portal",
  "dev_registered_commands",
]);

/**
 * `convertFileSrc` reads the Tauri internals object directly, so it throws
 * outside the webview rather than returning something useless. In DEV hand
 * back the bare path: the video won't load, which lands the player on its
 * error overlay, itself a state worth being able to look at.
 */
export function assetUrl(path: string): string {
  if (IN_TAURI) return convertFileSrc(path);
  return path;
}

/** The real transport. */
export const invokeTransport: Transport = {
  invoke(command, args) {
    if (DIRECT_COMMANDS.has(command)) return invoke(command, args);
    // `args` is forwarded untouched, so the wire shape is unchanged: the
    // camelCase the callers already send is what Rust now maps itself.
    return invoke("rpc", { command, args: args ?? {} });
  },
};
