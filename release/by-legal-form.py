"""Stands in for the extractor `spool init` writes, where a release is tested.

The sample extractor finds names with a small model, and its first run
fetches about 330 MB. This one finds what ends in a legal form, which in the
sample note is what the model finds, and has nothing to fetch but what every
Python file needs. It is the stand-in of
`crates/spool-cli/tests/first_extraction.rs`.
"""

import re

from spool import extractor

NAME = re.compile(r"(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b")


@extractor
def find(document):
    for found in NAME.finditer(document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
