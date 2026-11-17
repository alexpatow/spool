// Finds company names by their legal form: `suffix.py` in TypeScript. The two
// find the same mentions, and so make the same records.

import { extractor } from "spool-strategy";

const UPPER = "A-ZÀ-ÖØ-Þ";
const LETTER = "A-Za-zÀ-ÖØ-öø-ÿ";
const WORD = `[${UPPER}][${LETTER}0-9'-]*`;
const COMPANY = new RegExp(
  `(?:${WORD} (?:(?:&|de|und|of) )?)+(?:Inc|Ltd|LTD|Corp|GmbH|AG|SA)(?![${LETTER}0-9])`,
  "g",
);

await extractor(async (task) => {
  const documents = await task.documents();
  return documents.flatMap((document) =>
    [...document.text.matchAll(COMPANY)].map((found) =>
      document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
    ),
  );
});
