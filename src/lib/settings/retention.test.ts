import { describe, expect, it } from "vitest";
import { BYTES_PER_GB } from "../../format";
import type { EnforcementReport } from "../../types";
import {
  formToPolicy,
  limitsAnything,
  policyToForm,
  previewMessage,
  savedMessage,
} from "./retention";

/**
 * The retention form is the one place in the app where a careless edit
 * destroys footage, which is why it previews before it saves. These are the
 * conversions that sit either side of that.
 */

describe("formToPolicy", () => {
  it("converts gigabytes to bytes", () => {
    const policy = formToPolicy({
      sizeEnabled: true,
      sizeGb: "50",
      ageEnabled: false,
      ageDays: "",
    });
    expect(policy.max_total_bytes).toBe(50 * BYTES_PER_GB);
    expect(policy.max_age_days).toBeNull();
  });

  it("makes a disabled limit null, not zero", () => {
    // Zero would be a policy that deletes everything, which is the opposite
    // of "no limit".
    const policy = formToPolicy({
      sizeEnabled: false,
      sizeGb: "50",
      ageEnabled: false,
      ageDays: "30",
    });
    expect(policy.max_total_bytes).toBeNull();
    expect(policy.max_age_days).toBeNull();
  });

  it("keeps a limit the user typed but has not enabled out of the policy", () => {
    const policy = formToPolicy({
      sizeEnabled: false,
      sizeGb: "9999",
      ageEnabled: true,
      ageDays: "7",
    });
    expect(policy.max_total_bytes).toBeNull();
    expect(policy.max_age_days).toBe(7);
  });
});

describe("policyToForm", () => {
  it("round-trips with formToPolicy", () => {
    const form = { sizeEnabled: true, sizeGb: "120", ageEnabled: true, ageDays: "30" };
    expect(policyToForm(formToPolicy(form))).toEqual(form);
  });

  it("empties the field for a limit that is off", () => {
    // Not "0": the box should look untouched rather than set to something.
    expect(policyToForm({ max_total_bytes: null, max_age_days: null })).toEqual({
      sizeEnabled: false,
      sizeGb: "",
      ageEnabled: false,
      ageDays: "",
    });
  });
});

describe("limitsAnything", () => {
  it("is false only when both limits are off", () => {
    expect(limitsAnything({ sizeEnabled: false, sizeGb: "", ageEnabled: false, ageDays: "" })).toBe(
      false,
    );
    expect(limitsAnything({ sizeEnabled: true, sizeGb: "1", ageEnabled: false, ageDays: "" })).toBe(
      true,
    );
  });
});

const report = (deleted: number, freed: number): EnforcementReport =>
  ({
    deleted: Array.from({ length: deleted }, (_, i) => i),
    freed_bytes: freed,
  }) as unknown as EnforcementReport;

describe("previewMessage", () => {
  it("says what saving would destroy", () => {
    expect(previewMessage(report(3, 1024))).toContain("delete 3 recording(s)");
  });

  it("is null when nothing would go", () => {
    // The callout is hidden entirely. "Would delete 0 recordings" is a
    // sentence nobody needs to read every time they touch the field.
    expect(previewMessage(report(0, 0))).toBeNull();
  });
});

describe("savedMessage", () => {
  it("reports what actually went", () => {
    expect(savedMessage(report(2, 2048))).toContain("Deleted 2 recording(s)");
  });

  it("is null when a save deleted nothing", () => {
    expect(savedMessage(report(0, 0))).toBeNull();
  });
});
