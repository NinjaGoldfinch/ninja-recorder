import { afterEach, describe, expect, it, vi } from "vitest";
import { railOpenSaved, saveRailOpen } from "./rail";

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("the remembered rail", () => {
  it("is open until it is closed", () => {
    expect(railOpenSaved()).toBe(true);
    saveRailOpen(false);
    expect(railOpenSaved()).toBe(false);
    saveRailOpen(true);
    expect(railOpenSaved()).toBe(true);
  });

  it("opens when storage is unavailable, and saving does not throw", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(railOpenSaved()).toBe(true);
    expect(() => saveRailOpen(false)).not.toThrow();
  });
});
