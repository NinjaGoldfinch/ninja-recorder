/**
 * What the update row says, given a status.
 *
 * Moved out of `update.ts`'s `render` by WS4.4, which was one `switch` that
 * set text and toggled four elements' `hidden` and `disabled` at once. Pulling
 * the decision out is what makes the states testable without an updater.
 *
 * The module's own reason for being quiet is unchanged and lives in
 * `Update.svelte`: CI publishes a release for every commit on `main`, so
 * "something newer exists" is true most days.
 */

import type { UpdateStatus } from "../../types";

export interface UpdateRow {
  /** The sentence in the About block. */
  text: string;
  /** Whether an update is being offered, which drives the dot and the button. */
  offering: boolean;
  /** Whether the Install button should be usable. */
  installable: boolean;
  /** The release notes to show, or null when there are none to show. */
  notes: string | null;
}

export function updateRow(status: UpdateStatus): UpdateRow {
  switch (status.kind) {
    case "checking":
      return { text: "Checking…", offering: false, installable: false, notes: null };
    case "unsupported":
      // **Not "you are up to date"**: this build will never find out. A
      // devtools bundle and anything built off Windows both land here.
      return {
        text: "Updates are not available in this build.",
        offering: false,
        installable: false,
        notes: null,
      };
    case "upToDate":
      return { text: "Up to date.", offering: false, installable: false, notes: null };
    case "failed":
      // Printed verbatim: Rust sends a whole sentence, which is what lets a
      // failed check and a failed *install* share this one state without the
      // UI having to guess which it is looking at.
      return { text: status.error, offering: false, installable: false, notes: null };
    case "available": {
      const why = status.installable
        ? ""
        : ` Cannot install while ${status.blockedReason ?? "the app is busy"}.`;
      return {
        text: `Version ${status.offer.version} is available.${why}`,
        offering: true,
        installable: status.installable,
        notes: status.offer.notes,
      };
    }
  }
}
