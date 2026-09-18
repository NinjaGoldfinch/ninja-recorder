/**
 * Parsing the release notes CI writes into `latest.json`.
 *
 * **`latest.json` is fetched over HTTPS but is not covered by the update
 * signature** - only the installer it points at is - so everything here is
 * remote text this app did not write.
 *
 * `update.ts` rendered it by building DOM nodes with `createElement` and
 * `textContent`, which was the right call for a module that otherwise
 * assembled HTML strings: it meant there was no escaping to get wrong. This
 * returns a structure instead, and the component interpolates it, which gets
 * the same guarantee from the template rather than from a convention. **An
 * `{@html}` in the component would throw it away**, which is what
 * `notes.test.ts` is there to catch.
 *
 * The format CI writes is a `## What's changed` heading, `- ` bullets per
 * commit, and a trailing full-changelog line. The heading is dropped, because
 * the row is already labelled; bullets become a list; anything else is a
 * paragraph, so an unrecognised line is shown rather than swallowed.
 */

/** A run of text, optionally emphasised. */
export interface Span {
  text: string;
  strong: boolean;
}

export type Block = { kind: "list"; items: Span[][] } | { kind: "para"; spans: Span[] };

/**
 * Splits one line on `**bold**`.
 *
 * Emphasis is the only inline syntax that appears: GitHub's generated notes
 * end with `**Full changelog**: <url>`, and a paragraph rendered as plain text
 * shows those asterisks. Everything else is left exactly as written rather
 * than half-parsed.
 *
 * Non-greedy, and it cannot span lines because the caller has already split on
 * them, so an unclosed `**` matches nothing and the line is shown as written.
 */
export function inlineSpans(line: string): Span[] {
  const spans: Span[] = [];
  let last = 0;
  for (const match of line.matchAll(/\*\*(.+?)\*\*/g)) {
    const at = match.index ?? 0;
    if (at > last) spans.push({ text: line.slice(last, at), strong: false });
    spans.push({ text: match[1], strong: true });
    last = at + match[0].length;
  }
  if (last < line.length) spans.push({ text: line.slice(last), strong: false });
  return spans;
}

/**
 * The notes as blocks, or an empty array when there is nothing to show.
 *
 * Blank lines and `#` headings are dropped before anything else looks at them.
 * Consecutive bullets collect into one list; a non-bullet ends it.
 */
export function parseNotes(notes: string | null): Block[] {
  const lines = (notes ?? "")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"));

  const blocks: Block[] = [];
  for (const line of lines) {
    if (line.startsWith("- ")) {
      const last = blocks[blocks.length - 1];
      if (last?.kind === "list") last.items.push(inlineSpans(line.slice(2)));
      else blocks.push({ kind: "list", items: [inlineSpans(line.slice(2))] });
      continue;
    }
    blocks.push({ kind: "para", spans: inlineSpans(line) });
  }
  return blocks;
}
