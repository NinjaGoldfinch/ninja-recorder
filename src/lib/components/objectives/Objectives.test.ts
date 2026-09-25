import type { Component } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Objective } from "../../contract/types";

const client = vi.hoisted(() => ({
  list_objectives: vi.fn(),
  create_objective: vi.fn(),
  set_objective_status: vi.fn(),
  import_review_rows: vi.fn(),
}));
vi.mock("../../../bridge", () => ({ client, call: vi.fn(), assetUrl: (p: string) => p }));
vi.mock("../../../router", () => ({ showView: vi.fn(), registerView: vi.fn() }));
vi.mock("../../stores/daemon.svelte", () => ({ whenDaemonReachable: (fn: () => void) => fn() }));

const objective = (id: number, over: Partial<Objective> = {}): Objective => ({
  id,
  body: `objective ${id}`,
  category: "other",
  status: "active",
  created_at: id,
  retired_at: null,
  ...over,
});

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
type Svelte = typeof import("svelte");
let svelte: Svelte;
let Objectives: Component;

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

beforeEach(async () => {
  vi.resetModules();
  for (const fn of Object.values(client)) fn.mockReset();
  client.list_objectives.mockResolvedValue([
    objective(3, { body: "Promoted from a takeaway" }),
    objective(2, { status: "paused" }),
    objective(1, { status: "retired", retired_at: 10 }),
  ]);
  svelte = await import("svelte");
  Objectives = (await import("./Objectives.svelte")).default;
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

async function render(): Promise<HTMLElement> {
  instance = svelte.mount(Objectives, { target: host });
  await settle();
  return host;
}

const tab = (el: HTMLElement, label: string) =>
  [...el.querySelectorAll<HTMLButtonElement>('[aria-label="Status"] button')].find((b) =>
    b.textContent?.startsWith(label),
  );
const bodies = (el: HTMLElement) =>
  [...el.querySelectorAll(".objective-body")].map((n) => n.textContent);

describe("the objectives view", () => {
  it("lists every status once and shows the active ones first", async () => {
    const el = await render();
    expect(client.list_objectives).toHaveBeenCalledWith(null);
    expect(bodies(el)).toEqual(["Promoted from a takeaway"]);
    expect(tab(el, "Active")?.textContent).toBe("Active (1)");
    expect(tab(el, "Retired")?.textContent).toBe("Retired (1)");
  });

  it("switches tabs without asking the daemon again", async () => {
    const el = await render();
    tab(el, "Retired")?.click();
    await settle();
    expect(bodies(el)).toEqual(["objective 1"]);
    expect(client.list_objectives).toHaveBeenCalledTimes(1);
  });

  it("creates an active objective with the chosen category", async () => {
    client.create_objective.mockResolvedValue(
      objective(4, { body: "Ward at 2:45", category: "macro" }),
    );
    const el = await render();
    const input = el.querySelector<HTMLInputElement>('[aria-label="Objective"]');
    const select = el.querySelector<HTMLSelectElement>('[aria-label="Category"]');
    if (!input || !select) throw new Error("no form");
    input.value = "Ward at 2:45";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    select.value = "macro";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    el.querySelector<HTMLFormElement>(".objective-add")?.requestSubmit();
    await settle();

    expect(client.create_objective).toHaveBeenCalledWith("Ward at 2:45", "macro");
    expect(bodies(el)[0]).toBe("Ward at 2:45");
    expect(input.value).toBe("");
  });

  it("retires an objective, which moves it to the retired tab", async () => {
    client.set_objective_status.mockImplementation(async (id: number, status: string) =>
      objective(id, { body: "Promoted from a takeaway", status: status as Objective["status"] }),
    );
    const el = await render();
    [...el.querySelectorAll("button")].find((b) => b.textContent === "Retire")?.click();
    await settle();
    expect(client.set_objective_status).toHaveBeenCalledWith(3, "retired");
    expect(bodies(el)).toEqual([]);
    expect(tab(el, "Retired")?.textContent).toBe("Retired (2)");
  });

  it("imports a chosen CSV, reports what it did and names the lines it could not read", async () => {
    client.import_review_rows.mockImplementation(async (rows: unknown[]) => ({
      rows: rows.length,
      games_created: rows.length,
      games_matched: 0,
      objectives_created: 1,
      takeaways_created: 0,
      blocks_merged: 0,
    }));
    const el = await render();
    const input = el.querySelector<HTMLInputElement>('[aria-label="Spreadsheet CSV"]');
    if (!input) throw new Error("no file input");
    const csv =
      "date,time,game,learning objectives\n16/09/2026,5:23pm,loss,• Ward river\n16/09/2026,later,win,";
    Object.defineProperty(input, "files", {
      configurable: true,
      value: [new File([csv], "sheet.csv", { type: "text/csv" })],
    });
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();

    expect(client.import_review_rows).toHaveBeenCalledTimes(1);
    const [rows] = client.import_review_rows.mock.calls[0] as [
      { game: string; objectives: string[] }[],
    ];
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ game: "loss", objectives: ["Ward river"] });
    const outcome = el.querySelector(".import-outcome")?.textContent ?? "";
    expect(outcome).toContain("1 new games");
    expect(outcome).toContain('Line 3: "later" is not a time.');
    expect(
      client.list_objectives,
      "the list is reloaded to show what was imported",
    ).toHaveBeenCalledTimes(2);
  });

  it("sends nothing when no row could be read", async () => {
    const el = await render();
    const input = el.querySelector<HTMLInputElement>('[aria-label="Spreadsheet CSV"]');
    if (!input) throw new Error("no file input");
    Object.defineProperty(input, "files", {
      configurable: true,
      value: [new File(["when,where\n1,2"], "wrong.csv")],
    });
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await settle();
    expect(client.import_review_rows).not.toHaveBeenCalled();
    expect(el.querySelector(".import-errors")?.textContent).toContain('no "date" column');
  });
});
