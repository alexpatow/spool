import { describe, expect, test } from "bun:test";
import { listenOn, movePorts } from "./ports";

const page = "spool dev on http://127.0.0.1:7765/v1/info, a second on 127.0.0.1:7766, and 127.0.0.1:77650 is neither";

describe("a run on another port", () => {
  test("moves the two addresses of the pages, each once", () => {
    expect(movePorts(page, 7765, 7767)).toBe(
      "spool dev on http://127.0.0.1:7767/v1/info, a second on 127.0.0.1:7768, and 127.0.0.1:77650 is neither",
    );
  });

  test("does not move an address it has just written, when the new port is the page's second", () => {
    expect(movePorts(page, 7765, 7766)).toBe(
      "spool dev on http://127.0.0.1:7766/v1/info, a second on 127.0.0.1:7767, and 127.0.0.1:77650 is neither",
    );
  });

  test("is undone by the move back, for what a run printed", () => {
    for (const port of [7764, 7766, 7767, 9000]) {
      expect(movePorts(movePorts(page, 7765, port), port, 7765)).toBe(page);
    }
  });

  test("changes nothing on the pages' own port", () => {
    expect(movePorts(page, 7765, 7765)).toBe(page);
  });
});

describe("a server that names no address", () => {
  test("is told the run's port, whether it is to start or to be refused", () => {
    expect(listenOn("spool dev", 7765, 7791)).toBe("spool dev --listen 127.0.0.1:7791");
    expect(listenOn("spool dev ../other", 7765, 7791)).toBe("spool dev ../other --listen 127.0.0.1:7791");
    expect(listenOn("spool serve --store spool.duckdb", 7765, 7791)).toBe(
      "spool serve --store spool.duckdb --listen 127.0.0.1:7791",
    );
  });

  test("is the only line of a block that changes", () => {
    expect(listenOn("cd ../other\nspool dev\nspool show", 7765, 7791)).toBe(
      "cd ../other\nspool dev --listen 127.0.0.1:7791\nspool show",
    );
  });

  test("keeps the address it names, which the move of the ports has already seen to", () => {
    const named = "spool serve --store spool.duckdb --listen 127.0.0.1:7791";
    expect(listenOn(named, 7765, 7791)).toBe(named);
  });

  test("is left alone on the pages' own port", () => {
    expect(listenOn("spool dev ../other", 7765, 7765)).toBe("spool dev ../other");
  });

  test("is not a command that only names one", () => {
    expect(listenOn("echo spool dev", 7765, 7791)).toBe("echo spool dev");
    expect(listenOn("spool develop", 7765, 7791)).toBe("spool develop");
  });
});
