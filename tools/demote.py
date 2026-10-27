"""demote.py [--data DIR] [--diffs DIR] [--list]: measures, on the latest stored receipt of every PR, which looks the quiet rules turn into notes.

Receipt-level kinds (dependencies, fresh-release, agent-notes) are judged from the stored rules exactly as receipt::build now does.
Protected and settings looks need the file diffs, which stored receipts do not hold: --diffs names a folder of <receipt folder>.json
files holding GitHub's pulls/N/files answer; without it those two kinds are counted as kept."""
import collections, glob, json, os, re, sys

DATA = r"C:\Users\krish\Downloads\kern-dev\data"
DEPS = "live-deps"
PATTERN_KEYS = ("include", "testMatch")


def latest_receipts(root):
    latest = {}
    for d in os.listdir(root):
        for rd in glob.glob(os.path.join(root, d, "receipt-*")):
            p = os.path.join(rd, "receipt.json")
            if not os.path.isfile(p):
                continue
            pr = os.path.basename(rd)
            m = os.path.getmtime(p)
            if pr not in latest or m > latest[pr][0]:
                latest[pr] = (m, p, d)
    return latest


def uses_line(line):
    if "uses:" not in line:
        return None
    prefix, rest = line.split("uses:", 1)
    if prefix.lstrip(" \t-"):
        return None
    if "@" not in rest:
        return None
    action, at = rest.strip().split("@", 1)
    if not action or not at.strip():
        return None
    return prefix, action.strip(), at.strip()


def action_moves(patch):
    removed = [l[1:].rstrip() for l in patch.splitlines() if l.startswith("-") and not l.startswith("---")]
    added = [l[1:].rstrip() for l in patch.splitlines() if l.startswith("+") and not l.startswith("+++")]
    a, b = collections.Counter(removed), collections.Counter(added)
    removed = list((a - b).elements())
    added = list((b - a).elements())
    if not removed or len(removed) != len(added):
        return None
    moves = set()
    for line in removed:
        u = uses_line(line)
        if not u:
            return None
        match = next((i for i, l in enumerate(added) if uses_line(l) and uses_line(l)[0] == u[0] and uses_line(l)[1] == u[1]), None)
        if match is None:
            return None
        moves.add(f"{u[1]} {u[2]} -> {uses_line(added.pop(match))[2]}")
    return sorted(moves)


QUOTED = re.compile(r"""(['"`])([^'"`]*)\1""")


def added_patterns(patch):
    removed = [l[1:] for l in patch.splitlines() if l.startswith("-") and not l.startswith("---")]
    added = [l[1:] for l in patch.splitlines() if l.startswith("+") and not l.startswith("+++")]
    if not added:
        return None
    key = None
    for l in patch.splitlines():
        if l.startswith(("---", "+++")):
            continue
        named = re.match(r"[ +-]\s*([A-Za-z_]+)\s*:", l)
        if named:
            key = named.group(1)
        if l.startswith("+") and key not in PATTERN_KEYS:
            return None
    found = []
    old_literals = {m.group(2) for l in removed for m in QUOTED.finditer(l)}
    plain_added = [l for l in added if l.strip().lstrip(",").strip() == "" or re.fullmatch(r"\s*['\"`][^'\"`]+['\"`]\s*,?\s*", l)]
    if len(plain_added) == len(added):
        if removed:
            return None
        return [m.group(2) for l in added for m in QUOTED.finditer(l)] or None
    if len(removed) != len(added):
        return None
    for r, a in zip(removed, added):
        if not any(k in r for k in PATTERN_KEYS):
            return None
        new = [m for m in QUOTED.finditer(a) if m.group(2) not in old_literals]
        reduced = a
        for m in reversed(new):
            s, e = m.span()
            rest = reduced[e:]
            if rest.lstrip().startswith(","):
                e += len(rest) - len(rest.lstrip()) + 1
            else:
                before = reduced[:s].rstrip()
                if before.endswith(","):
                    s = len(before) - 1
            reduced = reduced[:s] + reduced[e:]
        if re.sub(r"\s+", "", reduced) != re.sub(r"\s+", "", r) or not new:
            return None
        found.extend(m.group(2) for m in new)
    return found or None


MANAGER = re.compile(r'"packageManager":\s*"([^"@]+)@')


def steering_fields(d, diff):
    fields = [f for f in d.get("fields") or [] if not (f == "name" and d.get("operation") == "add")]
    patch = (diff or {}).get(d.get("path", ""), "")
    if "packageManager" in fields and patch:
        removed = [m.group(1) for l in patch.splitlines() if l.startswith("-") for m in MANAGER.finditer(l)]
        added = [m.group(1) for l in patch.splitlines() if l.startswith("+") for m in MANAGER.finditer(l)]
        if removed and removed == added:
            fields.remove("packageManager")
    return fields


def after(r, diff):
    rules = r["rules"]
    kept, gone = [], []
    deps = rules.get("dependencies") or []
    for x in r.get("look") or []:
        k = x["kind"]
        if k in ("fresh-release", "agent-notes"):
            gone.append(x)
        elif k == "dependencies":
            if any(d.get("operation") == "delete" or steering_fields(d, diff) for d in deps):
                kept.append(dict(x, kind="install"))
            else:
                gone.append(x)
        elif k in ("protected", "settings") and diff is not None:
            path = x["what"].split("`")[1] if "`" in x["what"] else ""
            patch = diff.get(path)
            if patch is None:
                kept.append(x)
            elif k == "protected" and action_moves(patch):
                gone.append(x)
            elif k == "settings" and "changes the test runner's settings" in x["what"] and added_patterns(patch):
                gone.append(x)
            else:
                kept.append(x)
        else:
            kept.append(x)
    if r.get("blocked"):
        decision = "blocked"
    elif kept:
        decision = "look"
    elif r.get("unverified"):
        decision = "unverified"
    else:
        decision = "clear"
    return decision, kept, gone


def main():
    args = sys.argv[1:]
    root = args[args.index("--data") + 1] if "--data" in args else DATA
    diffs = args[args.index("--diffs") + 1] if "--diffs" in args else None
    listing = "--list" in args
    latest = latest_receipts(root)
    before = collections.Counter()
    after_count = collections.Counter()
    gone_kinds = collections.Counter()
    gone_prs = collections.Counter()
    kept_kinds = collections.Counter()
    deps_before = deps_after = 0
    looks_before = looks_after = 0
    changed = []
    for pr, (m, p, folder) in sorted(latest.items()):
        r = json.load(open(p, encoding="utf-8"))
        diff = None
        if diffs:
            f = os.path.join(diffs, pr + ".json")
            if os.path.isfile(f):
                diff = {x["filename"]: x.get("patch", "") for x in json.load(open(f, encoding="utf-8"))}
        decision, kept, gone = after(r, diff)
        before[r["decision"]] += 1
        after_count[decision] += 1
        if r.get("look"):
            looks_before += 1
        if kept:
            looks_after += 1
        if folder == DEPS:
            deps_before += bool(r.get("look"))
            deps_after += bool(kept)
        for x in gone:
            gone_kinds[x["kind"]] += 1
        for k in {x["kind"] for x in gone}:
            gone_prs[k] += 1
        for x in kept:
            kept_kinds[x["kind"]] += 1
        if decision != r["decision"] or gone:
            changed.append((pr, r["decision"], decision, [x["kind"] for x in gone], [x["what"] for x in gone]))
    print(f"PRs: {len(latest)}")
    print(f"decisions before: {dict(before)}")
    print(f"decisions after:  {dict(after_count)}")
    print(f"PRs with a look: {looks_before} -> {looks_after}")
    print(f"dependency PRs ({DEPS}) with a look: {deps_before} -> {deps_after}")
    print(f"blocked: {before['blocked']} -> {after_count['blocked']}")
    print(f"looks that become notes, by kind (items): {dict(gone_kinds)}")
    print(f"looks that become notes, by kind (PRs): {dict(gone_prs)}")
    print(f"looks kept, by kind (items): {dict(kept_kinds)}")
    print(f"PRs whose answer changes: {sum(1 for c in changed if c[1] != c[2])}")
    if listing:
        for pr, was, now, kinds, whats in changed:
            print(f"{pr}: {was} -> {now}; gone: {', '.join(kinds)}")
            for w in whats:
                print("    " + w)


if __name__ == "__main__":
    main()
