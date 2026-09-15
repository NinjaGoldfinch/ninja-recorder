/**
 * How the generated client reaches the daemon. WS2 task 2.6, WS3 task 3.6.
 *
 * Three implementations of one interface:
 *
 * - `invoke.ts` is Tauri `invoke('rpc', …)`, which is what v1 does today and
 *   what keeps working through WS2 before the daemon exists.
 * - `pipe.ts` will be JSON-RPC over the named pipe, the v2 target. Requests
 *   and notifications, with a snapshot on connect and on reconnect (WS3.4).
 * - `mock.ts` is in-memory, for Vitest and for the plain `vite` dev server.
 *
 * The client is written against the interface, so which one is in use is a
 * composition-root decision and not something a view knows (implementation
 * plan §4.2).
 */

/**
 * One call to the backend, by wire name.
 *
 * Structurally the generated client's own `Invoke` type, which is why
 * `createClient(t.invoke)` type-checks with no adapter. It is restated here as
 * an object rather than a bare function so an implementation can grow the
 * things a pipe needs (`subscribe`, `close`) without every caller changing
 * shape.
 */
export interface Transport {
  invoke(command: string, args: Record<string, unknown>): Promise<unknown>;
}
