import { describe, expect, test } from "bun:test";
import { asShown, noIds, sameOutput, tidy } from "./compare-output";

// What a page shows.
const A = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const B = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
// What a run printed.
const ONE = "11111111-1111-1111-1111-111111111111";
const TWO = "22222222-2222-2222-2222-222222222222";

describe("an ID the page shows twice", () => {
  test("fails when the run printed two different IDs in one block", () => {
    const ids = noIds();
    const page = [`crm:c-1  ${A}`, `crm:c-2  ${A}`];
    const run = [`crm:c-1  ${ONE}`, `crm:c-2  ${TWO}`];
    expect(sameOutput(page, run, ids)).toBe(false);
    expect(ids.shown.size).toBe(0);
    expect(ids.printed.size).toBe(0);
  });

  test("passes when the run printed one ID twice, and remembers it", () => {
    const ids = noIds();
    const page = [`crm:c-1  ${A}`, `crm:c-2  ${A}`];
    const run = [`crm:c-1  ${ONE}`, `crm:c-2  ${ONE}`];
    expect(sameOutput(page, run, ids)).toBe(true);
    expect(ids.shown.get(A)).toBe(ONE);
    expect(ids.printed.get(ONE)).toBe(A);
  });

  test("fails when the two are on one line", () => {
    expect(sameOutput([`merged ${A} into ${A}`], [`merged ${ONE} into ${TWO}`], noIds())).toBe(false);
  });

  test("fails in a later block", () => {
    const ids = noIds();
    expect(sameOutput([`crm:c-1  ${A}`], [`crm:c-1  ${ONE}`], ids)).toBe(true);
    expect(sameOutput([`crm:c-2  ${A}`], [`crm:c-2  ${TWO}`], ids)).toBe(false);
    expect(sameOutput([`crm:c-2  ${A}`], [`crm:c-2  ${ONE}`], ids)).toBe(true);
  });

  test("fails after `...` as well", () => {
    const ids = noIds();
    const page = [`crm:c-1  ${A}`, "...", `crm:c-9  ${A}`];
    const run = [`crm:c-1  ${ONE}`, `crm:c-5  ${TWO}`, `crm:c-9  ${TWO}`];
    expect(sameOutput(page, run, ids)).toBe(false);
    expect(ids.shown.size).toBe(0);
  });
});

describe("two IDs the page shows", () => {
  test("fail when the run printed one ID for both", () => {
    const page = [`crm:c-1  ${A}`, `crm:c-3  ${B}`];
    const run = [`crm:c-1  ${ONE}`, `crm:c-3  ${ONE}`];
    expect(sameOutput(page, run, noIds())).toBe(false);
  });

  test("pass when the run printed two", () => {
    const page = [`crm:c-1  ${A}`, `crm:c-3  ${B}`];
    const run = [`crm:c-1  ${ONE}`, `crm:c-3  ${TWO}`];
    expect(sameOutput(page, run, noIds())).toBe(true);
  });
});

describe("`...`", () => {
  test("stands for any lines, and none", () => {
    expect(sameOutput(["first", "...", "last"], ["first", "a", "b", "last"], noIds())).toBe(true);
    expect(sameOutput(["first", "...", "last"], ["first", "last"], noIds())).toBe(true);
    expect(sameOutput(["first", "...", "last"], ["first", "a"], noIds())).toBe(false);
  });

  test("keeps only the IDs of the lines that paired in the end", () => {
    const ids = noIds();
    // `row A` could pair with the first printed row, but then `end A` has nothing to pair with.
    const page = ["...", `row ${A}`, `end ${A}`];
    const run = [`row ${ONE}`, `row ${TWO}`, `end ${TWO}`];
    expect(sameOutput(page, run, ids)).toBe(true);
    expect(ids.shown.get(A)).toBe(TWO);
    expect(ids.printed.has(ONE)).toBe(false);
  });
});

test("times are compared by shape, and other text exactly", () => {
  const page = ["started 2026-11-18T10:00:00Z", "rows 3"];
  expect(sameOutput(page, ["started 2027-01-02T03:04:05.678Z", "rows 3"], noIds())).toBe(true);
  expect(sameOutput(page, ["started 2027-01-02T03:04:05.678Z", "rows 4"], noIds())).toBe(false);
  expect(sameOutput(page, ["started yesterday", "rows 3"], noIds())).toBe(false);
});

test("a background command is compared with what it printed first", () => {
  expect(sameOutput(["listening"], ["listening", "applied"], noIds(), true)).toBe(true);
  expect(sameOutput(["listening"], ["listening", "applied"], noIds())).toBe(false);
});

test("an update keeps the ID the pages already show", () => {
  const ids = noIds();
  expect(sameOutput([`crm:c-1  ${A}`], [`crm:c-1  ${ONE}`], ids)).toBe(true);
  expect(asShown(`crm:c-4  ${ONE}`, ids)).toBe(`crm:c-4  ${A}`);
  expect(asShown(`crm:c-5  ${TWO}`, ids)).toBe(`crm:c-5  ${TWO}`);
  // From here on the new ID stands for itself.
  expect(sameOutput([`crm:c-5  ${TWO}`], [`crm:c-5  ${TWO}`], ids)).toBe(true);
  expect(sameOutput([`crm:c-5  ${TWO}`], [`crm:c-5  ${ONE}`], ids)).toBe(false);
});

test("tidy drops trailing spaces and trailing empty lines", () => {
  expect(tidy("a  \n\nb\n\n\n")).toEqual(["a", "", "b"]);
});

describe("the number of a process", () => {
  test("is compared by its shape", () => {
    const page = ["the store is open in another process (PID 3635), most likely a `spool dev`"];
    const run = ["the store is open in another process (PID 91), most likely a `spool dev`"];
    expect(sameOutput(page, run, noIds())).toBe(true);
  });

  test("leaves other numbers to be compared as they are", () => {
    expect(sameOutput(["run 4: 1 key named"], ["run 5: 1 key named"], noIds())).toBe(false);
  });
});
