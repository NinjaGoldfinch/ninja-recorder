/**
 * How the generated client reaches the daemon — WS2 task 2.6, WS3 task 3.6.
 *
 * Three implementations of one interface:
 *
 * - `invoke.ts` — Tauri `invoke('rpc', …)`, which is what v1 does today and
 *   what keeps working through WS2 before the daemon exists.
 * - `pipe.ts` — JSON-RPC over the named pipe, the v2 target. Requests and
 *   notifications, with a snapshot on connect and on reconnect.
 * - `mock.ts` — in-memory, for Vitest. The reason component tests need
 *   neither a daemon nor a WebView2.
 *
 * The client is written against the interface, so which one is in use is a
 * composition-root decision, not something a view knows (implementation plan
 * §4.2).
 */
export {};
