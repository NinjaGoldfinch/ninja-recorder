import { describe, expect, it } from "vitest";
import type { BackfillReport } from "../../types";
import { backfillSummary } from "./backfill";

const report = (over: Partial<BackfillReport> = {}): BackfillReport =>
  ({
    scanned: 0,
    games_considered: 0,
    patched: 0,
    gold_filled: 0,
    ambiguous: 0,
    unmatched: 0,
    ...over,
  }) as BackfillReport;

describe("backfillSummary", () => {
  it("separates 'nothing to do' from 'nothing worked'", () => {
    // The whole reason this function says more than it has to. The two look
    // identical if only successes are reported, and they call for opposite
    // reactions.
    expect(backfillSummary(report({ scanned: 0 }))).toContain("already has its match data");
    expect(backfillSummary(report({ scanned: 3, games_considered: 20 }))).toContain(
      "Nothing could be filled in.",
    );
  });

  it("counts what it filled in", () => {
    const text = backfillSummary(report({ scanned: 5, games_considered: 20, patched: 3 }));
    expect(text).toContain("Checked 5 recordings against 20 games");
    expect(text).toContain("Filled in 3.");
  });

  it("gets the singulars right", () => {
    expect(backfillSummary(report({ scanned: 1, games_considered: 1 }))).toContain(
      "Checked 1 recording against 1 game of match history",
    );
  });

  it("counts the gold curve separately", () => {
    // A different endpoint that fails independently: a game can yield its
    // metadata after the timeline has aged out of the client's history.
    expect(backfillSummary(report({ scanned: 2, patched: 1, gold_filled: 1 }))).toContain(
      "Recovered the gold curve for 1.",
    );
  });

  it("says when it refused to guess", () => {
    // Refusing to guess is a decision, not a failure.
    expect(backfillSummary(report({ scanned: 2, ambiguous: 2 }))).toContain(
      "overlapped more than one game and were left alone",
    );
  });

  it("explains an unmatched row rather than leaving it a mystery", () => {
    expect(backfillSummary(report({ scanned: 2, unmatched: 2 }))).toContain(
      "older than your client's history, or were customs",
    );
  });
});
