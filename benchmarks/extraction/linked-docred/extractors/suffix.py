"""Finds organisations by the word their name ends in.

A run of capitalised words that ends in a legal form (`Inc.`, `Ltd`, `GmbH`)
or in a word that says what kind of body it is (`University`, `Party`,
`Records`) is taken to be an organisation. It is the extractor of
`../company-news` with a longer list of endings, and it is a baseline: the
least an extractor can do without a model.

It is wrong in the ways rules of this kind are wrong. A name that does not
end in one of the words is missed, which is most bands, teams and companies
as an encyclopedia writes them. A name that begins with the word is missed
too ("University of ..."). A capitalised word before a name is taken for
part of it, unless it is one of the few words below that begin sentences.

The lists were written from what such names look like in general, and not
from the dataset this benchmark runs on.
"""

import re

from spool import extractor

UPPER = "A-ZÀ-ÖØ-Þ"
LETTER = "A-Za-zÀ-ÖØ-öø-ÿ"
WORD = f"[{UPPER}][{LETTER}0-9'-]*"

# Each is a regular expression. An abbreviation keeps the full stop it is
# written with.
LEGAL_FORMS = r"""
    Inc\.? Ltd\.? LTD Limited LLC LLP plc PLC Corp\.? Corporation Incorporated
    Co\. Company GmbH AG SA S\.A\. NV N\.V\. AB Oy SpA
""".split()
KINDS = """
    University College Institute Academy School Association Society Foundation
    Federation Committee Council Commission Party Union League Club Bank Group
    Records Airlines Airways Railway Railroad Agency Ministry Department
    Church Army Navy Orchestra Studios Entertainment Pictures Press Network
    Organization Organisation Authority Brothers Industries Motors Systems
    Technologies Holdings Partners
""".split()

# Words that are capitalised because a sentence begins with them.
NOT_A_NAME = set(
    """
    The A An In On At By For From With After Before When While Since During
    Until Under As And But Or He She It They His Her Its Their This That
    These Those
    """.split()
)

ENDING = "|".join(LEGAL_FORMS + KINDS)
ORGANISATION = re.compile(
    f"(?:{WORD} (?:(?:&|de|und|of) )?)*{WORD},? (?:{ENDING})(?![{LETTER}0-9])"
)


@extractor
def find(document):
    for found in ORGANISATION.finditer(document.text):
        start, name = found.start(), found.group()
        # "The Acme Company" is "Acme Company", and "In Paris Acme Ltd" is
        # still wrong: only the first word is looked at.
        first, _, rest = name.partition(" ")
        if first in NOT_A_NAME and rest:
            start, name = start + len(first) + 1, rest
            if " " not in name:
                continue
        yield document.mention("organisation", start, found.end(), name=name, document=document.key)
