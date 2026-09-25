/**
 * The objectives list (WS9 P0).
 *
 * Moves on command results, like `gameReview.svelte.ts` and for the same
 * reason: nothing else writes objectives yet. Holds every status at once and
 * filters locally, because the list is a handful of rows and switching tabs
 * should not wait on the daemon.
 */

import { client } from "../../bridge";
import type { Objective, ObjectiveCategory, ObjectiveStatus } from "../contract/types";
import { toast } from "./toast.svelte";

let all = $state<Objective[]>([]);
let filter = $state<ObjectiveStatus>("active");
let loaded = $state(false);

export const objectives = {
  get all() {
    return all;
  },
  get filter() {
    return filter;
  },
  set filter(next: ObjectiveStatus) {
    filter = next;
  },
  get loaded() {
    return loaded;
  },
  /** The rows under the current tab, newest first as the daemon sends them. */
  get visible() {
    return all.filter((o) => o.status === filter);
  },
  count(status: ObjectiveStatus) {
    return all.filter((o) => o.status === status).length;
  },
};

export async function loadObjectives(): Promise<void> {
  try {
    all = await client.list_objectives(null);
    loaded = true;
  } catch (err) {
    toast(`Couldn't load objectives: ${err}`, "error");
  }
}

export async function createObjective(body: string, category: ObjectiveCategory): Promise<boolean> {
  if (body.trim() === "") return false;
  try {
    const created = await client.create_objective(body, category);
    all = [created, ...all];
    return true;
  } catch (err) {
    toast(`Couldn't add the objective: ${err}`, "error");
    return false;
  }
}

export async function setObjectiveStatus(id: number, status: ObjectiveStatus): Promise<void> {
  try {
    const updated = await client.set_objective_status(id, status);
    all = all.map((o) => (o.id === id ? updated : o));
  } catch (err) {
    toast(`Couldn't update the objective: ${err}`, "error");
  }
}

/** Test seam: back to a freshly loaded module. */
export function resetObjectivesForTests(): void {
  all = [];
  filter = "active";
  loaded = false;
}
