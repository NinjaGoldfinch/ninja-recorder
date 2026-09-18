/**
 * The retention form: the one place in the app where a careless edit destroys
 * footage.
 *
 * Moved out of `settings.ts` by WS4.4, where reading the policy and writing it
 * back were two functions that talked to four inputs.
 */

import { BYTES_PER_GB, formatBytes } from "../../format";
import type { EnforcementReport, RetentionPolicy } from "../../types";

/** The form's state, which is not the same shape as the policy. */
export interface RetentionForm {
  sizeEnabled: boolean;
  /** Gigabytes, as typed. Empty when the limit is off. */
  sizeGb: string;
  ageEnabled: boolean;
  /** Days, as typed. Empty when the limit is off. */
  ageDays: string;
}

/**
 * The policy a form describes.
 *
 * A disabled limit is `null`, not zero: zero would be a policy that deletes
 * everything, which is the opposite of "no limit".
 */
export function formToPolicy(form: RetentionForm): RetentionPolicy {
  return {
    max_total_bytes: form.sizeEnabled ? Math.round(Number(form.sizeGb) * BYTES_PER_GB) : null,
    max_age_days: form.ageEnabled ? Number(form.ageDays) : null,
  };
}

/** The form a policy describes. Round-trips with `formToPolicy`. */
export function policyToForm(policy: RetentionPolicy): RetentionForm {
  return {
    sizeEnabled: policy.max_total_bytes !== null,
    sizeGb:
      policy.max_total_bytes !== null
        ? String(Math.round(policy.max_total_bytes / BYTES_PER_GB))
        : "",
    ageEnabled: policy.max_age_days !== null,
    ageDays: policy.max_age_days !== null ? String(policy.max_age_days) : "",
  };
}

/** Whether a form is asking for anything at all, which is what decides if a
 *  preview is worth requesting. */
export function limitsAnything(form: RetentionForm): boolean {
  return form.sizeEnabled || form.ageEnabled;
}

/**
 * What saving this policy would delete, or null when it would delete nothing.
 *
 * Null rather than an empty string: the callout is hidden entirely in that
 * case, and "would delete 0 recordings" is a sentence nobody needs to read
 * every time they touch the field.
 */
export function previewMessage(report: EnforcementReport): string | null {
  if (report.deleted.length === 0) return null;
  return (
    `Saving this will delete ${report.deleted.length} recording(s) ` +
    `and free ${formatBytes(report.freed_bytes)}.`
  );
}

/** What saving it actually did, or null when it deleted nothing. */
export function savedMessage(report: EnforcementReport): string | null {
  if (report.deleted.length === 0) return null;
  return `Deleted ${report.deleted.length} recording(s), freed ${formatBytes(report.freed_bytes)}.`;
}
