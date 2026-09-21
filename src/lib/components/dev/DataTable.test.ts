import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import DataTable from "./DataTable.svelte";

/**
 * The portal's grid renders database rows and command output, which is backend
 * data this frontend did not write. `ui.ts` escaped every cell and every
 * `title` by hand; these are the tests that keep that true now it is a
 * template.
 */

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

type Props = Parameters<typeof DataTable>[1];

function render(props: Props) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(DataTable, { target: host, props });
  return host;
}

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("DataTable", () => {
  it("says so when there is nothing, in the caller's words", () => {
    const el = render({ columns: ["id"], rows: [], emptyMessage: "No recordings seeded." });
    expect(el.querySelector("table")).toBeNull();
    expect(el.textContent).toContain("No recordings seeded.");
  });

  it("renders a header and a row per record", () => {
    const el = render({
      columns: ["id", "path"],
      rows: [
        [1, "a.mp4"],
        [2, "b.mp4"],
      ],
    });
    expect(el.querySelectorAll("th")).toHaveLength(2);
    expect(el.querySelectorAll("tbody tr")).toHaveLength(2);
  });

  it("renders null as the word, not as an empty cell", () => {
    // In a debug table the difference between "no value" and "empty string" is
    // usually the thing being looked for.
    const el = render({ columns: ["champion"], rows: [[null], [""]] });
    const cells = el.querySelectorAll("tbody td");
    expect(cells[0].textContent).toBe("null");
    expect(cells[0].className).toContain("null");
    expect(cells[1].textContent?.trim()).toBe("");
  });

  it("right-aligns only the columns it was told to", () => {
    const el = render({
      columns: ["path", "size"],
      rows: [["a.mp4", 1024]],
      numericColumns: new Set(["size"]),
    });
    const cells = el.querySelectorAll("tbody td");
    expect(cells[0].className).not.toContain("num");
    expect(cells[1].className).toContain("num");
  });

  it("serialises an object cell rather than printing [object Object]", () => {
    const el = render({ columns: ["payload"], rows: [[{ kind: "kill" }]] });
    expect(el.querySelector("tbody td")?.textContent?.trim()).toBe('{"kind":"kill"}');
  });

  it("reports which row was clicked, by index", () => {
    const onrow = vi.fn();
    const el = render({ columns: ["id"], rows: [[1], [2], [3]], onrow });
    el.querySelectorAll<HTMLElement>("tbody tr")[2].click();
    expect(onrow).toHaveBeenCalledWith(2);
  });

  it("is not clickable without a handler", () => {
    const el = render({ columns: ["id"], rows: [[1]] });
    expect(el.querySelector("tbody tr")?.className).not.toContain("clickable");
  });

  it("renders a cell that looks like markup as text, in the body and the title", () => {
    const nasty = "<img src=x onerror=alert(1)>";
    const el = render({ columns: ["path"], rows: [[nasty]] });

    expect(el.querySelectorAll("img")).toHaveLength(0);
    const cell = el.querySelector("tbody td");
    expect(cell?.textContent?.trim()).toBe(nasty);
    expect(cell?.getAttribute("title")).toBe(nasty);
  });
});
