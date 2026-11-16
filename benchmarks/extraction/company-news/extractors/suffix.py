"""Finds company names by their legal form.

A run of capitalised words that ends in `Ltd`, `Inc`, `Corp`, `GmbH`, `AG` or
`SA` is taken to be a company. That is right often enough to be a baseline
and wrong in the ways rules of this kind are wrong: a name written without
its legal form is missed, a capitalised word before the name is taken for
part of it, and "the Falcon Ltd edition" is taken for a company.

`suffix_ts.ts` is the same extractor in TypeScript.
"""

import re

from spool import extractor

UPPER = "A-ZÀ-ÖØ-Þ"
LETTER = "A-Za-zÀ-ÖØ-öø-ÿ"
WORD = f"[{UPPER}][{LETTER}0-9'-]*"
COMPANY = re.compile(
    f"(?:{WORD} (?:(?:&|de|und|of) )?)+(?:Inc|Ltd|LTD|Corp|GmbH|AG|SA)(?![{LETTER}0-9])"
)


@extractor
def find(document):
    for found in COMPANY.finditer(document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
