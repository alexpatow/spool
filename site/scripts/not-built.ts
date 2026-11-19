/**
 * What the pages describe that is not built yet.
 *
 * The pages are written for version 0.1.0, and some of what they show does not
 * exist. The samples check reports each use of such a thing under its name
 * here, in one list that is kept apart from real mismatches. The list is done
 * when it is empty.
 *
 * A block in a page carries `sample: not built <name>`. It is run all the
 * same. If it works, the check says the mark can go and compares the block
 * as any other.
 *
 * An entry with `run: false` cannot be tried by the check at all, so its
 * blocks are listed until the mark is taken out by hand.
 */
export interface NotBuilt {
  what: string;
  run?: false;
}

export const notBuilt: Record<string, NotBuilt> = {
  "agent-run": {
    what: "An agent has followed Getting started over MCP, and the page names the agent and its version",
    run: false,
  },
};
