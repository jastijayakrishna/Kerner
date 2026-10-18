import collections
import json
import pathlib
import sys
from benchmark import read, write


def receipts(root):
    out = {}
    for path in sorted(pathlib.Path(root).glob("farm-art*/receipt-*/receipt.json")):
        key = f"{path.parent.parent.name}/{path.parent.name}"
        out[key] = read(path)
    return out


def compare(before, after, source=None):
    a, b = receipts(before), receipts(after)
    selected = {f"{p.parent.parent.name}/{p.parent.name}" for p in pathlib.Path(source).glob("farm-art*/receipt-*/receipt.json")} if source else set(a) | set(b)
    keys = sorted(selected)
    moves, rows, pairs = collections.Counter(), [], set()
    for key in keys:
        old, new = a.get(key), b.get(key)
        if old is None or new is None:
            rows.append({"key": key, "unsupported": "Missing replay result", "before": old is not None, "after": new is not None})
            continue
        if (old["base"], old["head"]) != (new["base"], new["head"]):
            raise ValueError(f"Different revisions for {key}")
        pairs.add((old["base"], old["head"]))
        moves[(old["decision"], new["decision"])] += 1
        old_kinds = {field: [x.get("kind") for x in old.get(field, [])] for field in ("blocked", "look", "unverified")}
        new_kinds = {field: [x.get("kind") for x in new.get(field, [])] for field in ("blocked", "look", "unverified")}
        if old["decision"] != new["decision"] or old_kinds != new_kinds:
            rows.append({"key": key, "before": old["decision"], "after": new["decision"], "before_kinds": old_kinds, "after_kinds": new_kinds})
    return {"starting_commit": "479030dd39e464e0c8eb9cbb58b10c9ce3b802f5", "stored_checks": len(keys), "baseline_results": len(a), "repaired_results": len(b),
        "paired_results": sum(moves.values()), "uncompleted_checks": len(keys) - sum(moves.values()), "unique_commit_pairs": len(pairs),
        "transitions": [{"before": x, "after": y, "count": count} for (x, y), count in sorted(moves.items())],
        "new_blocked": [r["key"] for r in rows if r.get("after") == "blocked" and r.get("before") != "blocked"],
        "new_clear": [r["key"] for r in rows if r.get("after") == "clear" and r.get("before") != "clear"], "changed": rows,
        "limits": ["Stored checks are repeated measurements, not independent PRs.", "Replays rejudge saved runtime evidence; they do not rerun project code or recompute static rules.",
            "These are repair-audit results, not a live AI comparison, adjudicated bug counts, or proof of future correctness."]}


if __name__ == "__main__":
    result = compare(sys.argv[1], sys.argv[2], sys.argv[4] if len(sys.argv) > 4 else None)
    write(sys.argv[3], result)
    print(json.dumps({k: v for k, v in result.items() if k not in ("changed", "limits")}))
