import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The updater's two rules that are easy to state and easy to break.
 *
 * A download in flight owns the row, so a background check landing mid-install
 * must not put the buttons back under the user. And opening Settings checks,
 * but not on every visit, because Settings is one click from the library.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../bridge", () => ({ call, hasDevCommands: vi.fn(), assetUrl: (p: string) => p }));
const toast = vi.hoisted(() => vi.fn());
vi.mock("../../toast", () => ({ toast }));

let store: typeof import("./update.svelte");

beforeEach(async () => {
  vi.resetModules();
  call.mockReset();
  toast.mockReset();
  store = await import("./update.svelte");
});

afterEach(() => {
  vi.useRealTimers();
});

describe("refreshUpdateStatus", () => {
  it("reports what the backend says", async () => {
    call.mockResolvedValue({ kind: "upToDate" });
    await store.refreshUpdateStatus();
    expect(store.update.row?.text).toBe("Up to date.");
    expect(store.update.offering).toBe(false);
  });

  it("offers an available update", async () => {
    call.mockResolvedValue({
      kind: "available",
      installable: true,
      offer: { version: "2.1.0", notes: null },
    });
    await store.refreshUpdateStatus();
    expect(store.update.offering).toBe(true);
    expect(store.update.row?.installable).toBe(true);
  });

  it("says nothing louder than a console line when the command itself fails", async () => {
    // A build with no updater answers `unsupported` rather than rejecting, so
    // reaching here means something else broke. Nobody asked.
    call.mockRejectedValue(new Error("no daemon"));
    await store.refreshUpdateStatus();
    expect(toast).not.toHaveBeenCalled();
    expect(store.update.row).toBeNull();
  });
});

describe("a download in flight", () => {
  it("is not interrupted by a background check", async () => {
    call.mockResolvedValue(undefined);
    const installing = store.install();
    await installing;
    expect(store.update.installing).toBe(true);

    // The six-hourly loop lands mid-download and says there is an update.
    call.mockResolvedValue({
      kind: "available",
      installable: true,
      offer: { version: "2.1.0", notes: null },
    });
    await store.refreshUpdateStatus();

    // The row must not go back to offering an Install button.
    expect(store.update.installing).toBe(true);
    expect(store.update.offering).toBe(false);
  });

  it("does give way to a failure, which is the install's own outcome", async () => {
    call.mockResolvedValue(undefined);
    await store.install();
    expect(store.update.installing).toBe(true);

    call.mockResolvedValue({ kind: "failed", error: "Download failed: 404" });
    await store.refreshUpdateStatus();

    expect(store.update.installing).toBe(false);
    expect(store.update.row?.text).toBe("Download failed: 404");
  });

  it("reports a refusal, because the user pressed a button", async () => {
    // A game started between the render and the click, which is the case the
    // backend's second gate check exists for.
    call.mockRejectedValueOnce(new Error("a recording is in progress"));
    call.mockResolvedValue({ kind: "upToDate" });
    await store.install();

    expect(toast).toHaveBeenCalledWith(
      expect.stringContaining("recording is in progress"),
      "error",
    );
    expect(store.update.installing).toBe(false);
  });
});

describe("checking on open", () => {
  it("checks the first time Settings is opened", async () => {
    call.mockResolvedValue({ kind: "upToDate" });
    await store.checkOnOpen();
    expect(call).toHaveBeenCalledWith("check_for_update");
  });

  it("does not check again straight away", async () => {
    // Settings is one click from the library and gets revisited; a bare
    // "check on open" would be a request per visit.
    call.mockResolvedValue({ kind: "upToDate" });
    await store.onStatusEvent();
    call.mockClear();

    await store.checkOnOpen();
    expect(call).not.toHaveBeenCalledWith("check_for_update");
  });

  it("checks again once the answer is stale", async () => {
    call.mockResolvedValue({ kind: "upToDate" });
    await store.onStatusEvent();
    call.mockClear();

    vi.useFakeTimers();
    vi.setSystemTime(Date.now() + 120_000);
    await store.checkOnOpen();
    expect(call).toHaveBeenCalledWith("check_for_update");
  });

  it("never checks while a download is running", async () => {
    call.mockResolvedValue(undefined);
    await store.install();
    call.mockClear();
    call.mockResolvedValue({ kind: "checking" });

    await store.checkOnOpen();
    expect(call).not.toHaveBeenCalledWith("check_for_update");
  });
});

describe("changing channel", () => {
  it("rechecks immediately rather than describing the channel they left", async () => {
    call.mockResolvedValue({ kind: "upToDate" });
    await store.onStatusEvent();
    call.mockClear();

    await store.recheckForChannel();
    expect(call).toHaveBeenCalledWith("check_for_update");
  });
});
