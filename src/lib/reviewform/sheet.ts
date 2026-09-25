/**
 * Reading the review spreadsheet's CSV export (WS9 §3.2).
 *
 * Parsed here rather than in the daemon because this is where the file is
 * chosen and where the local timezone lives: a row says "16/09/2026, 5:23pm",
 * and only the machine's own clock rules, DST included, can say which moment
 * that was. `Date`'s local constructor applies them. The daemon receives typed
 * rows and does the matching and deduplication (`db::review_import`).
 *
 * Every row that cannot be read becomes an error naming its line, and the
 * rest are still imported: one bad date in a season of games should not cost
 * the season.
 */

import type { ImportRow } from "../contract/types";
import { parseClock, parseCount } from "./fields";

export interface SheetError {
  line: number;
  message: string;
}

export interface Sheet {
  rows: ImportRow[];
  errors: SheetError[];
}

/** A record and the 1-based line it starts on. */
interface CsvRecord {
  line: number;
  cells: string[];
}

/**
 * RFC 4180, which is what every spreadsheet writes: quoted cells may hold
 * commas, newlines and doubled quotes. The bullet lists live in exactly such
 * cells, so splitting on newlines first would cut rows in half.
 */
export function readCsv(text: string): CsvRecord[] {
  const source = text.replace(/^﻿/, "");
  const records: CsvRecord[] = [];
  let cells: string[] = [];
  let cell = "";
  let quoted = false;
  let line = 1;
  let start = 1;

  const endCell = () => {
    cells.push(cell);
    cell = "";
  };
  const endRecord = () => {
    endCell();
    records.push({ line: start, cells });
    cells = [];
  };

  for (let i = 0; i < source.length; i++) {
    const ch = source[i];
    if (quoted) {
      if (ch === '"' && source[i + 1] === '"') {
        cell += '"';
        i++;
      } else if (ch === '"') {
        quoted = false;
      } else {
        if (ch === "\n") line++;
        cell += ch;
      }
    } else if (ch === '"') {
      quoted = true;
    } else if (ch === ",") {
      endCell();
    } else if (ch === "\n" || ch === "\r") {
      if (ch === "\r" && source[i + 1] === "\n") i++;
      endRecord();
      line++;
      start = line;
    } else {
      cell += ch;
    }
  }
  if (cell !== "" || cells.length > 0) endRecord();
  return records;
}

/** The columns the export has, by the name the header gives them. */
const COLUMNS = [
  "date",
  "time",
  "block",
  "game_no",
  "playing",
  "matchup",
  "game",
  "lane",
  "mental",
  "clear_time",
  "smites",
  "deaths",
  "learning_objectives",
  "key_takeaways",
  "block_takeaways",
] as const;
type Column = (typeof COLUMNS)[number];

function columnName(header: string): string {
  return header
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, "_");
}

/** A bullet list in one cell: one item per line or per bullet glyph. */
export function splitBullets(cell: string): string[] {
  return cell
    .split(/\r?\n|•/)
    .map((item) => item.replace(/^\s*(?:[-*·–]|\d+[.)])\s+/, "").trim())
    .filter((item) => item !== "");
}

const WORDS: Partial<Record<string, string>> = {
  w: "win",
  won: "win",
  victory: "win",
  l: "loss",
  lost: "loss",
  lose: "loss",
  defeat: "loss",
  n: "neutral",
  even: "neutral",
  g: "good",
  b: "bad",
};

/** A rating cell, or `undefined` if it is none of `allowed`. Blank is `null`. */
function rating<T extends string>(cell: string, allowed: readonly T[]): T | null | undefined {
  const word = cell.trim().toLowerCase();
  if (word === "") return null;
  const value = WORDS[word] ?? word;
  return (allowed as readonly string[]).includes(value) ? (value as T) : undefined;
}

/**
 * Day first: `16/09/2026`, `16/9/26`, with or without a weekday in front, or
 * ISO `2026-09-16`. A date with no year is refused rather than guessed.
 */
export function parseDate(cell: string): { y: number; m: number; d: number } | null {
  const text = cell.trim().replace(/^[A-Za-z]{3,9},?\s+/, "");
  let match = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(text);
  if (match) return valid(Number(match[1]), Number(match[2]), Number(match[3]));
  match = /^(\d{1,2})\/(\d{1,2})\/(\d{2}|\d{4})$/.exec(text);
  if (!match) return null;
  const year = match[3].length === 2 ? 2000 + Number(match[3]) : Number(match[3]);
  return valid(year, Number(match[2]), Number(match[1]));
}

function valid(y: number, m: number, d: number) {
  if (m < 1 || m > 12 || d < 1 || d > 31) return null;
  return { y, m, d };
}

/** `17:23`, `5:23pm`, `5:23 PM`. */
export function parseTime(cell: string): { h: number; min: number } | null {
  const match = /^(\d{1,2}):(\d{2})\s*([ap]m)?$/i.exec(cell.trim());
  if (!match) return null;
  let h = Number(match[1]);
  const min = Number(match[2]);
  const meridiem = match[3]?.toLowerCase();
  if (min > 59) return null;
  if (meridiem) {
    if (h < 1 || h > 12) return null;
    h = (h % 12) + (meridiem === "pm" ? 12 : 0);
  } else if (h > 23) {
    return null;
  }
  return { h, min };
}

/** Local wall-clock time to unix millis, by the machine's own rules. */
export function localMillis(y: number, m: number, d: number, h: number, min: number): number {
  return new Date(y, m - 1, d, h, min).getTime();
}

export function parseSheet(text: string, toMillis = localMillis): Sheet {
  const [header, ...records] = readCsv(text);
  if (!header) return { rows: [], errors: [{ line: 1, message: "The file is empty." }] };
  const index = new Map<string, number>();
  header.cells.forEach((name, i) => {
    index.set(columnName(name), i);
  });
  for (const required of ["date", "time"] as const) {
    if (!index.has(required)) {
      return { rows: [], errors: [{ line: 1, message: `There is no "${required}" column.` }] };
    }
  }

  const rows: ImportRow[] = [];
  const errors: SheetError[] = [];
  for (const record of records) {
    if (record.cells.every((c) => c.trim() === "")) continue;
    const get = (column: Column) => {
      const i = index.get(column);
      return i === undefined ? "" : (record.cells[i] ?? "");
    };
    const fail = (message: string) => errors.push({ line: record.line, message });

    const date = parseDate(get("date"));
    if (!date) {
      fail(`"${get("date")}" is not a date with a year, day first.`);
      continue;
    }
    const time = parseTime(get("time"));
    if (!time) {
      fail(`"${get("time")}" is not a time.`);
      continue;
    }
    const game = rating(get("game"), ["win", "loss"] as const);
    const lane = rating(get("lane"), ["win", "neutral", "loss"] as const);
    const mental = rating(get("mental"), ["good", "neutral", "bad"] as const);
    if (game === undefined) {
      fail(`"${get("game")}" is not win or loss.`);
      continue;
    }
    if (lane === undefined) {
      fail(`"${get("lane")}" is not win, neutral or loss.`);
      continue;
    }
    if (mental === undefined) {
      fail(`"${get("mental")}" is not good, neutral or bad.`);
      continue;
    }
    const clear = parseClock(get("clear_time"));
    const smites = parseCount(get("smites"));
    const deaths = parseCount(get("deaths"));
    if (!clear.ok) {
      fail(`"${get("clear_time")}" is not a clear time (m:ss).`);
      continue;
    }
    if (!smites.ok || !deaths.ok) {
      fail("Smites and deaths must be whole numbers.");
      continue;
    }
    const text = (column: Column) => get(column).trim() || null;

    rows.push({
      line: record.line,
      started_at: toMillis(date.y, date.m, date.d, time.h, time.min),
      block: text("block"),
      champion: text("playing"),
      matchup: text("matchup"),
      game,
      lane,
      mental,
      clear_ms: clear.value,
      smites: smites.value,
      deaths: deaths.value,
      objectives: splitBullets(get("learning_objectives")),
      takeaways: splitBullets(get("key_takeaways")),
      block_takeaways: splitBullets(get("block_takeaways")),
    });
  }
  return { rows, errors };
}
