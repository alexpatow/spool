"""The rules of this entity type, written in Python.

It judges the candidates the block rules found exactly as the SQL files under
`rules/` do, so a run with this strategy commits what a run with the built-in
`rules` commits. That is what it is for: it shows that user code goes through
the same path as the rules, and what a strategy file looks like.

    spool bench benchmarks/identity/companies-small --strategy in_python
"""

from spool import strategy


def same(left, right):
    return left is not None and left == right


@strategy
def judge(candidates):
    judgements = []
    for pair in candidates.to_pylist():
        left = {name[5:]: value for name, value in pair.items() if name.startswith("left_")}
        right = {name[6:]: value for name, value in pair.items() if name.startswith("right_")}

        # rules/no_match: two registry identifiers that differ are two legal
        # entities, however alike the names and domains are.
        different = []
        if (
            left["registry_id"] is not None
            and right["registry_id"] is not None
            and left["registry_id"] != right["registry_id"]
        ):
            different.append("different_registry_id")

        # rules/match
        agree = []
        if same(left["domain"], right["domain"]):
            agree.append("same_domain")
        if same(left["name_key"], right["name_key"]) and same(left["country"], right["country"]):
            agree.append("same_name_and_country")
        if same(left["registry_id"], right["registry_id"]):
            agree.append("same_registry_id")
        if (
            same(left["name_key"], right["trading_key"])
            or same(left["trading_key"], right["name_key"])
        ) and same(left["country"], right["country"]):
            agree.append("trading_name_and_country")

        if different or agree:
            judgements.append(
                {
                    "left_id": pair["left_id"],
                    "right_id": pair["right_id"],
                    "verdict": "no_match" if different else "match",
                    "detail": {"no_match": different, "match": agree},
                }
            )
    return judgements
