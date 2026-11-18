import { describe, expect, test } from "bun:test";
import { movePorts } from "./ports";

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
