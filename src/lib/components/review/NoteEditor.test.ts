import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import NoteEditor from "./NoteEditor.svelte";

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

function render(onsave = vi.fn(), body = "went in without vision") {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(NoteEditor, {
    target: host,
    props: { label: "Note at 3:22", body, onsave, oncancel: () => {} },
  });
  flushSync();
  return host;
}

const enter = (target: Element, shiftKey = false) =>
  target.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", shiftKey, bubbles: true, cancelable: true }),
  );

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("NoteEditor", () => {
  it("saves on Enter after the kind is changed, with the focus still on the dropdown", () => {
    const onsave = vi.fn();
    const el = render(onsave);
    const picker = el.querySelector<HTMLSelectElement>('[aria-label="Kind"]');
    if (!picker) throw new Error("no kind picker");
    picker.value = "question";
    picker.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();

    enter(picker);
    expect(onsave).toHaveBeenCalledWith("question", "went in without vision");
  });

  it("saves on Enter in the text box, and Shift+Enter does not", () => {
    const onsave = vi.fn();
    const text = render(onsave).querySelector<HTMLTextAreaElement>('[aria-label="Note"]');
    if (!text) throw new Error("no text box");
    enter(text, true);
    expect(onsave).not.toHaveBeenCalled();
    enter(text);
    expect(onsave).toHaveBeenCalledWith("note", "went in without vision");
  });

  it("leaves Enter on Cancel to Cancel", () => {
    const onsave = vi.fn();
    const cancel = [...render(onsave).querySelectorAll("button")].find(
      (b) => b.textContent?.trim() === "Cancel",
    );
    if (!cancel) throw new Error("no cancel button");
    enter(cancel);
    expect(onsave).not.toHaveBeenCalled();
  });
});
