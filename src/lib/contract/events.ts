// GENERATED FILE. Do not edit by hand.
//
// Regenerate with:  cargo run --bin gen-contract   (from src-tauri/)
// CI runs the same binary with --check and fails if this file is stale.
//
// The declaration lives in Rust:
//   commands  src-tauri/src/core/dispatch.rs   (dispatch_table!)
//   events    src-tauri/src/contract/events.rs (contract_events!)
//   types     src-tauri/src/contract/types.rs  (the boundary list)

import type { Event, Topic } from "./types";

export type { Event, Topic };

/**
 * Which topic carries which event.
 *
* A client subscribes by topic, never by event name, which is what lets
* a variant be added to an existing topic without a client change.
 */
export const EVENTS_BY_TOPIC = {
  recording: ["stateChanged", "recordingStarted", "recordingStopped", "markerAdded", "sampleBatch"],
  lcu: ["lcuPhase"],
  library: ["matchSummaryPatched", "libraryChanged", "retentionRan"],
  update: ["updateStatus"],
  daemon: ["daemonShuttingDown", "lagged"],
} as const;
