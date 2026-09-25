/**
 * Debounced autosave for the review form.
 *
 * The form saves the **whole** review each time (`save_game_review` replaces
 * it), so the queue never needs to merge anything: it only needs to make sure
 * the last value typed is the last value written. Two rules do that:
 *
 * - **One save in flight at a time.** A change that lands while a save is
 *   running waits for it and then saves the newest value, never an older one.
 * - **A failed save stays unsaved.** The status says so, and the next change
 *   or `flush` tries again with the current value.
 *
 * No clock and no DOM: timers go through `setTimeout`, which the tests fake.
 */

export type SaveStatus = "saved" | "unsaved" | "saving" | "error";

export interface Autosave<T> {
  /** Record a new value and save it after the quiet period. */
  change(value: T): void;
  /** Save now if anything is unsaved, and resolve once it has been. */
  flush(): Promise<void>;
  /** Drop any pending save without writing it. */
  cancel(): void;
}

export interface AutosaveOptions<T> {
  save: (value: T) => Promise<void>;
  delayMs: number;
  onStatus: (status: SaveStatus, error?: unknown) => void;
}

export function createAutosave<T>({ save, delayMs, onStatus }: AutosaveOptions<T>): Autosave<T> {
  let pending: { value: T } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inFlight: Promise<void> | null = null;

  async function run(): Promise<void> {
    while (pending) {
      const { value } = pending;
      pending = null;
      onStatus("saving");
      try {
        await save(value);
      } catch (error) {
        // Put it back unless something newer arrived meanwhile, in which
        // case the newer value is the one worth retrying.
        pending ??= { value };
        onStatus("error", error);
        return;
      }
    }
    onStatus("saved");
  }

  function start(): Promise<void> {
    clearTimeout(timer);
    timer = undefined;
    if (!inFlight) {
      inFlight = run().finally(() => {
        inFlight = null;
      });
    }
    return inFlight;
  }

  return {
    change(value) {
      pending = { value };
      onStatus("unsaved");
      clearTimeout(timer);
      // A save already running picks this up when it finishes.
      if (!inFlight) timer = setTimeout(() => void start(), delayMs);
    },
    async flush() {
      if (inFlight) await inFlight;
      if (pending) await start();
    },
    cancel() {
      clearTimeout(timer);
      timer = undefined;
      pending = null;
    },
  };
}
