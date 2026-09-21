/**
 * The SQL console's snippets, and the text-to-value rule the row editor uses.
 *
 * Both were inline in the Database panel, which is the one panel where a
 * mistake is a write against the live library.
 */

/**
 * Text → the JSON value the backend binds. **Empty means NULL**, which is why
 * the editor's placeholder says so on a nullable column.
 *
 * Everything else is a guess, and the order of the guesses is the contract:
 * an unparseable object literal falls back to text rather than being rejected,
 * because a column that holds a string starting with `{` is legal and the
 * editor should not be the thing that refuses it.
 */
export function parseCell(raw: string): unknown {
  const text = raw.trim();
  if (text === "") return null;
  if (text === "true") return true;
  if (text === "false") return false;
  if (/^-?\d+(\.\d+)?$/.test(text)) return Number(text);
  if (text.startsWith("{") || text.startsWith("[")) {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }
  return text;
}

export type Snippet = [name: string, sql: string];

export const SNIPPET_KEY = "ninja-dev-sql-snippets";

export const DEFAULT_SNIPPETS: Snippet[] = [
  [
    "Recordings with markers",
    "SELECT r.id, r.champion, COUNT(m.id) AS markers\nFROM recordings r LEFT JOIN markers m ON m.recording_id = r.id\nGROUP BY r.id ORDER BY markers DESC",
  ],
  ["Marker kinds", "SELECT kind, COUNT(*) AS n FROM markers GROUP BY kind ORDER BY n DESC"],
  [
    "Orphaned markers",
    "SELECT * FROM markers WHERE recording_id NOT IN (SELECT id FROM recordings)",
  ],
  [
    "Size by pinned",
    "SELECT pinned, COUNT(*) AS n, SUM(size_bytes) AS bytes FROM recordings GROUP BY pinned",
  ],
];

/** Never throws: a corrupt or unavailable `localStorage` costs the saved
 *  snippets, not the panel. */
export function savedSnippets(): Snippet[] {
  try {
    const raw = localStorage.getItem(SNIPPET_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (s): s is Snippet =>
        Array.isArray(s) && s.length === 2 && typeof s[0] === "string" && typeof s[1] === "string",
    );
  } catch {
    return [];
  }
}

export function saveSnippet(name: string, sql: string): Snippet[] {
  const next: Snippet[] = [...savedSnippets(), [name, sql]];
  try {
    localStorage.setItem(SNIPPET_KEY, JSON.stringify(next));
  } catch {
    // Out of quota or blocked. The snippet is lost, the query is not.
  }
  return next;
}

export function allSnippets(): Snippet[] {
  return [...DEFAULT_SNIPPETS, ...savedSnippets()];
}
