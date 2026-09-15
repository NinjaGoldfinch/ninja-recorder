/**
 * The frontend's one way to reach the backend.
 *
 * Since WS2.6 this is a thin composition root rather than an implementation:
 * it picks a transport (`lib/transport/`) and exposes the generated client
 * (`lib/contract/`) over it. The Tauri `invoke` plumbing moved to
 * `transport/invoke.ts` and the several hundred lines of dev fixtures moved to
 * `transport/mock.ts`, which is what lets Vitest drive the real client with
 * neither a daemon nor a WebView2 behind it.
 *
 * `call` survives unchanged, because the existing views are written against it
 * and WS2.6's exit criterion is that frontend behaviour does not move. New
 * code should prefer `client`, which is typed per command; `call` is what WS4
 * strangles as each view is rewritten.
 */

import { createClient } from "./lib/contract/client";
import type { Transport } from "./lib/transport/index";
import { assetUrl, IN_TAURI, invokeTransport } from "./lib/transport/invoke";
import { mockTransport } from "./lib/transport/mock";

export { assetUrl };

/**
 * Which transport this session is using.
 *
 * The mock is reachable only from the vite dev server, never from a shipped
 * build: `import.meta.env.DEV` is statically false in production, so the
 * import above tree-shakes out along with every fixture behind it.
 */
const transport: Transport = IN_TAURI
  ? invokeTransport
  : import.meta.env.DEV
    ? mockTransport
    : {
        invoke(command) {
          throw new Error(`invoke("${command}") outside Tauri`);
        },
      };

export function isMocked(): boolean {
  return !IN_TAURI && import.meta.env.DEV;
}

/**
 * The generated, per-command typed client.
 *
 * Every method's name, arguments and return type come from the Rust
 * declaration by way of `gen-contract`, so a command that changes shape is a
 * `tsc` error at the call site rather than a runtime surprise.
 */
export const client = createClient((command, args) => transport.invoke(command, args));

/**
 * The untyped escape hatch, kept for the views that predate the generated
 * client.
 *
 * Identical behaviour to the v1 version: same transport, same wire shape, same
 * rejection outside Tauri. It is deliberately still here rather than rewritten
 * away in this task, because "frontend behaviour unchanged" is the exit
 * criterion and touching nine view files would not be that.
 */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return transport.invoke(command, args ?? {}) as Promise<T>;
}

/**
 * Whether this build carries the `devtools` Cargo feature, decided the only
 * way the frontend can decide it: ask whether the `dev_*` commands are
 * registered. A shipped build rejects the call, and that rejection *is* the
 * answer, so this resolves `false` rather than throwing.
 *
 * Memoised because two callers want it, `devportal.ts` to reveal the portal
 * button and `desktop.ts` to leave the webview's own context menu and reload
 * key alone in a build that can inspect, and one probe is enough.
 */
let devCommands: Promise<boolean> | null = null;

export function hasDevCommands(): Promise<boolean> {
  devCommands ??= call<string[]>("dev_registered_commands")
    .then((commands) => commands.length > 0)
    .catch(() => false);
  return devCommands;
}
