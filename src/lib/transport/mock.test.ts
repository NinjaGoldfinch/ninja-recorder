/**
 * The mock transport, driven through the generated client. WS2 task 2.6.
 *
 * This is the pairing the task exists for: the real `createClient` output,
 * over the in-memory transport, with neither a daemon nor a WebView2 behind
 * it. Before WS2.6 neither half could be exercised from a test at all, because
 * the fixtures were private to `bridge.ts` and reachable only by being outside
 * Tauri with `import.meta.env.DEV` set.
 *
 * It is also the cheapest guard on the exit criterion. "Frontend behaviour
 * unchanged" is mostly a claim about wiring, and the way wiring breaks is a
 * command reaching the transport under the wrong name or with the wrong
 * argument shape.
 */

import { beforeEach, describe, expect, it } from "vitest";

import { createClient } from "../contract/client";
import type { Transport } from "./index";
import { mockTransport } from "./mock";

const client = createClient((command, args) => mockTransport.invoke(command, args));

describe("the generated client over the mock transport", () => {
  it("answers a read with the fixture rows", async () => {
    const rows = await client.list_recordings();
    expect(rows.length).toBeGreaterThan(0);
    expect(rows[0]).toHaveProperty("champion");
  });

  it("returns disk usage that agrees with the rows it reports", async () => {
    const rows = await client.list_recordings();
    const usage = await client.get_disk_usage();
    expect(usage.recording_count).toBe(rows.length);
    expect(usage.total_bytes).toBe(rows.reduce((a, r) => a + r.size_bytes, 0));
  });

  /**
   * Writes mutate the fixtures rather than no-op'ing, which is what makes a
   * two-step delete worth testing at all.
   */
  it("persists a write for the life of the session", async () => {
    const [first] = await client.list_recordings();
    const before = first.pinned;
    await client.set_pinned(first.id, !before);
    const [again] = await client.list_recordings();
    expect(again.pinned).toBe(!before);
    await client.set_pinned(first.id, before);
  });

  it("removes a row on delete", async () => {
    const rows = await client.list_recordings();
    const victim = rows[rows.length - 1];
    await client.delete_recording(victim.id);
    const after = await client.list_recordings();
    expect(after.map((r) => r.id)).not.toContain(victim.id);
  });

  /**
   * Rejecting rather than no-op'ing is deliberate: outside the webview there
   * is nothing to restart into, and a button that silently "worked" would be
   * the one piece of this flow a browser session could not tell apart from
   * the real thing.
   */
  it("refuses an install rather than pretending", async () => {
    await expect(client.install_update()).rejects.toThrow(/not available/i);
  });

  it("says so for a command it has no fixture for", async () => {
    await expect(mockTransport.invoke("no_such_command", {})).rejects.toThrow(/no dev fixture/i);
  });
});

describe("the review over the mock transport", () => {
  it("opens one game per recording and keeps what is saved to it", async () => {
    const [row] = await client.list_recordings();
    const gameId = await client.open_game_for_recording(row.id);
    expect(await client.open_game_for_recording(row.id)).toBe(gameId);

    const review = await client.get_game_review(gameId);
    expect(review?.objectives.length).toBeGreaterThan(0);
    await client.save_game_review(gameId, {
      game_rating: "win",
      lane_rating: null,
      mental_rating: "good",
      first_clear_ms: 178_000,
      smites_at_clear: 1,
      deaths: null,
      free_notes: "",
    });
    expect((await client.get_game_review(gameId))?.review?.game_rating).toBe("win");
  });

  it("promotes a takeaway into an active objective once", async () => {
    const [row] = await client.list_recordings();
    const gameId = await client.open_game_for_recording(row.id);
    const takeaway = await client.add_takeaway({ kind: "game", id: gameId }, "Contest grubs");
    const objective = await client.promote_takeaway(takeaway.id, "macro");
    expect((await client.promote_takeaway(takeaway.id, "macro")).id).toBe(objective.id);
    const active = await client.list_objectives("active");
    expect(active.map((o) => o.body)).toContain("Contest grubs");
  });
});

describe("the argument names the client sends", () => {
  /**
   * The one thing that breaks silently. `rename_all = "camelCase"` on the
   * generated `Args` struct means Rust parses `recordingId`; a client sending
   * `recording_id` would deserialize to a default and the command would act on
   * the wrong row. The mock reads the same key, so this pins both halves.
   */
  let seen: { command: string; args: Record<string, unknown> } | null = null;
  const spy: Transport = {
    invoke(command, args) {
      seen = { command, args };
      return Promise.resolve(null);
    },
  };
  const spied = createClient((command, args) => spy.invoke(command, args));

  beforeEach(() => {
    seen = null;
  });

  it("camelCases an argument, and leaves the command name alone", async () => {
    await spied.get_recording_markers(42);
    expect(seen).toEqual({ command: "get_recording_markers", args: { recordingId: 42 } });
  });

  it("sends every argument of a multi-argument command", async () => {
    await spied.set_pinned(7, true);
    expect(seen).toEqual({ command: "set_pinned", args: { recordingId: 7, pinned: true } });
  });

  it("sends an empty object for a command that takes nothing", async () => {
    await spied.list_recordings();
    expect(seen).toEqual({ command: "list_recordings", args: {} });
  });
});
