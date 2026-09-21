import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * Which commands go through `rpc` and which are invoked directly.
 *
 * The list is three names long and one of them is load-bearing in a way that
 * fails silently: routing `dev_registered_commands` through `rpc` would make
 * it reject with "unknown command" in *every* build rather than only in a
 * shipped one, which permanently hides the Dev portal button instead of
 * hiding it where it should be hidden.
 */

const invoke = vi.hoisted(() => vi.fn());
const convertFileSrc = vi.hoisted(() => vi.fn((p: string) => `asset://${p}`));
vi.mock("@tauri-apps/api/core", () => ({ invoke, convertFileSrc }));

let transport: typeof import("./invoke");

beforeEach(async () => {
  vi.resetModules();
  invoke.mockReset();
  invoke.mockResolvedValue(null);
  transport = await import("./invoke");
});

describe("routing", () => {
  it("wraps an ordinary command in the rpc passthrough", async () => {
    await transport.invokeTransport.invoke("list_recordings", { limit: 10 });
    expect(invoke).toHaveBeenCalledWith("rpc", {
      command: "list_recordings",
      args: { limit: 10 },
    });
  });

  it("still sends an args field for a command that takes none", async () => {
    // Rust parses `args` on the way through, so the field has to be there.
    // The generated client passes `{}` rather than omitting it, which is why
    // `Transport.invoke` takes an object rather than an optional one.
    await transport.invokeTransport.invoke("get_disk_usage", {});
    expect(invoke).toHaveBeenCalledWith("rpc", { command: "get_disk_usage", args: {} });
  });

  it("forwards arguments untouched, so the wire shape is unchanged", async () => {
    await transport.invokeTransport.invoke("set_pinned", { recordingId: 7, pinned: true });
    expect(invoke.mock.calls[0][1]).toEqual({
      command: "set_pinned",
      args: { recordingId: 7, pinned: true },
    });
  });

  it.each(["open_recordings_folder", "dev_open_portal", "dev_registered_commands"])(
    "invokes %s directly",
    async (command) => {
      await transport.invokeTransport.invoke(command, {});
      expect(invoke).toHaveBeenCalledWith(command, {});
      expect(invoke).not.toHaveBeenCalledWith("rpc", expect.anything());
    },
  );

  it("keeps the direct list to the three that need it", async () => {
    // A fourth name added here is a command the daemon then never sees.
    for (const command of ["rescan_recordings", "dev_health", "get_autostart"]) {
      invoke.mockClear();
      await transport.invokeTransport.invoke(command, {});
      expect(invoke.mock.calls[0][0], command).toBe("rpc");
    }
  });
});

describe("assetUrl", () => {
  it("hands back the bare path outside the webview", () => {
    // `convertFileSrc` reads Tauri's internals object directly, so it throws
    // rather than returning something useless. The video then fails to load,
    // which lands the player on its error overlay: a state worth being able
    // to look at in a browser.
    expect(transport.assetUrl("C:\\Videos\\a.mp4")).toBe("C:\\Videos\\a.mp4");
    expect(convertFileSrc).not.toHaveBeenCalled();
  });
});
