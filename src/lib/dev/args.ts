/**
 * Turning the generated command catalogue's argument specs into an `invoke`
 * payload.
 *
 * The catalogue is generated from the Rust declaration (WS2.7), so the specs
 * are trustworthy; what is not trustworthy is what someone typed into the
 * form. This is where the two meet, and it used to be inline in a panel that
 * also built the form's HTML and read the values back out of the DOM.
 */

import type { PortalArg } from "../../dev/commands.generated";

export type CollectResult =
  | { ok: true; args: Record<string, unknown> }
  | { ok: false; error: string };

/** A field's value before coercion. Always a string: a checkbox holds
 *  `"true"` / `"false"` so that one map covers every kind. */
export type RawArgs = Record<string, string>;

/** What a field starts as. The generator writes `""` for "no default", which
 *  is also a valid default for a string argument - the two are the same thing
 *  here, since an empty optional is omitted and an empty required is refused. */
export function defaultFor(spec: PortalArg): string {
  if (spec.default !== "") return spec.default;
  return spec.kind === "boolean" ? "false" : "";
}

/**
 * The payload for a command, or the first thing wrong with the form.
 *
 * **An omitted optional argument is absent, not null.** Several commands
 * distinguish "not given", which means keep the saved value, from an explicit
 * null, which means clear it. Sending `null` for an empty box would quietly
 * turn every optional field into a delete.
 */
export function collectArgs(specs: readonly PortalArg[], raw: RawArgs): CollectResult {
  const args: Record<string, unknown> = {};

  for (const spec of specs) {
    const value = raw[spec.name] ?? defaultFor(spec);

    if (spec.kind === "boolean") {
      args[spec.name] = value === "true";
      continue;
    }

    const trimmed = value.trim();
    if (trimmed === "") {
      if (spec.optional) continue;
      return { ok: false, error: `${spec.name} is required` };
    }

    if (spec.kind === "number") {
      const n = Number(trimmed);
      if (Number.isNaN(n)) return { ok: false, error: `${spec.name} is not a number` };
      args[spec.name] = n;
    } else if (spec.kind === "json") {
      try {
        args[spec.name] = JSON.parse(trimmed);
      } catch (err) {
        return { ok: false, error: `${spec.name} is not valid JSON: ${err}` };
      }
    } else {
      args[spec.name] = trimmed;
    }
  }

  return { ok: true, args };
}
