/**
 * The dev portal's shared state - #72.
 *
 * **One poll, fanned out.** The backend answers `dev_health` with a single
 * round trip, and a page where five panels each started their own timer would
 * throw that away. `main.ts` ran it once and pushed into whichever panel was
 * showing; here it is state, and any component that reads it re-renders.
 *
 * `env` is read once at boot and never changes. Its failure is the one worth
 * spelling out: reaching `dev.html` in a build without the `devtools` feature
 * makes every panel show an opaque "command not found", so the boot records
 * the error and the shell says what to run instead.
 */

import { tryCall } from "../../dev/ipc";
import type { DevEnvInfo, DevHealth } from "../../dev/types";

let health = $state<DevHealth | null>(null);
let healthError = $state<string | null>(null);
let env = $state<DevEnvInfo | null>(null);
let envError = $state<string | null>(null);
let live = $state(true);

let timer: ReturnType<typeof setInterval> | null = null;

export const dev = {
  get health() {
    return health;
  },
  get healthError() {
    return healthError;
  },
  get env() {
    return env;
  },
  /** Set when `dev_env_info` failed, which in practice means no devtools. */
  get envError() {
    return envError;
  },
  get live() {
    return live;
  },
};

/** The database path, split so only the directory half can shrink. */
export function splitDbPath(path: string): { dir: string; file: string } {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut === -1 ? { dir: "", file: path } : { dir: path.slice(0, cut), file: path.slice(cut) };
}

export async function loadEnv(): Promise<void> {
  const result = await tryCall<DevEnvInfo>("dev_env_info");
  if (result.ok) {
    env = result.value;
    envError = null;
  } else {
    env = null;
    envError = result.error;
  }
}

async function poll(): Promise<void> {
  const result = await tryCall<DevHealth>("dev_health");
  if (result.ok) {
    health = result.value;
    healthError = null;
  } else {
    health = null;
    healthError = result.error;
  }
}

export function setLive(on: boolean): void {
  live = on;
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
  if (on) {
    void poll();
    timer = setInterval(() => void poll(), 1000);
  }
}

export function stopPolling(): void {
  if (timer !== null) clearInterval(timer);
  timer = null;
}
