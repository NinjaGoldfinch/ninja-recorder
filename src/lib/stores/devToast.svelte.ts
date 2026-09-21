/**
 * The dev portal's toast stack - #72.
 *
 * A stack rather than one message, unlike the app's: the portal fires several
 * in a row when a batch of writes lands, and replacing each with the next
 * would leave only the last one readable.
 */

export type DevToastTone = "" | "ok" | "err";

export interface DevToast {
  id: number;
  message: string;
  tone: DevToastTone;
}

let items = $state<DevToast[]>([]);
let nextId = 0;

export const devToasts = {
  get items() {
    return items;
  },
};

export function devToast(message: string, tone: DevToastTone = "") {
  const id = nextId++;
  items = [...items, { id, message, tone }];
  setTimeout(() => {
    items = items.filter((t) => t.id !== id);
  }, 5200);
}

/** For teardown, and for tests. */
export function clearDevToasts() {
  items = [];
}
