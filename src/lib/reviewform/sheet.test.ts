import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { localMillis, parseDate, parseSheet, parseTime, readCsv, splitBullets } from "./sheet";

/** A clock with no timezone, so the test says the same thing everywhere. */
const utc = (y: number, m: number, d: number, h: number, min: number) =>
  Date.UTC(y, m - 1, d, h, min);

const FIXTURE = readFileSync("fixtures/review/spreadsheet.csv", "utf8");

describe("readCsv", () => {
  it("keeps commas, newlines and doubled quotes inside a quoted cell", () => {
    const [row] = readCsv('a,"one, two\nthree ""quoted""",c');
    expect(row.cells).toEqual(["a", 'one, two\nthree "quoted"', "c"]);
  });

  it("numbers each record by the line it starts on, across multi-line cells", () => {
    const records = readCsv('h\r\n"x\ny"\r\nz');
    expect(records.map((r) => r.line)).toEqual([1, 2, 4]);
  });

  it("ignores a byte-order mark", () => {
    expect(readCsv("﻿date,time")[0].cells).toEqual(["date", "time"]);
  });
});

describe("the cells", () => {
  it("splits a bullet list by line and by bullet, and drops the glyphs", () => {
    expect(splitBullets("• one\n• two")).toEqual(["one", "two"]);
    expect(splitBullets("- one\n2. two\n\n")).toEqual(["one", "two"]);
    expect(splitBullets("• one • two")).toEqual(["one", "two"]);
    expect(splitBullets("")).toEqual([]);
  });

  it("reads dates day first, with or without a weekday, and refuses one with no year", () => {
    expect(parseDate("Wed 16/09/2026")).toEqual({ y: 2026, m: 9, d: 16 });
    expect(parseDate("1/2/26")).toEqual({ y: 2026, m: 2, d: 1 });
    expect(parseDate("2026-09-16")).toEqual({ y: 2026, m: 9, d: 16 });
    expect(parseDate("16/09")).toBeNull();
    expect(parseDate("09/16/2026"), "there is no month 16").toBeNull();
  });

  it("reads 12- and 24-hour times", () => {
    expect(parseTime("5:23pm")).toEqual({ h: 17, min: 23 });
    expect(parseTime("12:05 AM")).toEqual({ h: 0, min: 5 });
    expect(parseTime("12:05pm")).toEqual({ h: 12, min: 5 });
    expect(parseTime("19:02")).toEqual({ h: 19, min: 2 });
    expect(parseTime("25:00")).toBeNull();
    expect(parseTime("13:00pm")).toBeNull();
  });

  it("uses the machine's own clock for local time", () => {
    expect(localMillis(2026, 9, 16, 17, 23)).toBe(new Date(2026, 8, 16, 17, 23).getTime());
  });
});

describe("parseSheet on the fixture", () => {
  const sheet = parseSheet(FIXTURE, utc);

  it("reads every good row and names the line of the bad one", () => {
    expect(sheet.rows).toHaveLength(3);
    expect(sheet.errors).toEqual([{ line: 10, message: '"bad time" is not a time.' }]);
  });

  it("turns the first row into a typed import row", () => {
    expect(sheet.rows[0]).toEqual({
      line: 2,
      started_at: Date.UTC(2026, 8, 16, 17, 23),
      block: "1",
      champion: "Lee Sin",
      matchup: "Vi",
      game: "loss",
      lane: "neutral",
      mental: "good",
      clear_ms: 178_000,
      smites: 1,
      deaths: 7,
      objectives: ["Ward river at 2:45", "Track the enemy jungler's first clear"],
      takeaways: ["Contested grubs with no prio, lost both", 'Died to the level 3 invade, "again"'],
      block_takeaways: ["Stop after two losses in a row"],
    });
  });

  it("reads shorthand ratings and leaves blank numbers unset", () => {
    expect(sheet.rows[1].game).toBe("win");
    expect(sheet.rows[1].takeaways).toEqual(["Took small win top and rotated"]);
    expect(sheet.rows[2]).toMatchObject({
      lane: "loss",
      clear_ms: null,
      smites: null,
      deaths: null,
    });
    expect(sheet.rows[2].block_takeaways).toEqual([]);
  });
});

describe("parseSheet refusals", () => {
  it("names a rating it does not know rather than guessing", () => {
    const sheet = parseSheet("date,time,game\n16/09/2026,17:00,draw", utc);
    expect(sheet.rows).toEqual([]);
    expect(sheet.errors).toEqual([{ line: 2, message: '"draw" is not win or loss.' }]);
  });

  it("refuses a file with no date column", () => {
    expect(parseSheet("when,time\n1,2", utc).errors[0].message).toMatch(/no "date" column/);
  });
});
