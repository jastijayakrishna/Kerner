import argparse
import hashlib
import json
import math
import pathlib
import re
import statistics
import subprocess

ROWS_COUNT_MAX = 2000
JSON_BYTES_MAX = 16 << 20
SOURCE_BYTES_MAX = 512 << 10
FILES_COUNT_MAX = 60
SALT = "kern-counterexample-v1/"
FIELDS = ("repo", "number", "base_sha", "head_sha", "merge_base")


def read(path):
    path = pathlib.Path(path)
    if path.stat().st_size > JSON_BYTES_MAX:
        raise ValueError("JSON exceeds 16 MiB")
    return json.loads(path.read_text(encoding="utf-8-sig"))


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def write(path, value):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def identity(row):
    repo, number = row["repo"], row["number"]
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("Invalid repository")
    if type(number) is not int or number <= 0:
        raise ValueError("Invalid PR number")
    for key in ("base_sha", "head_sha", *(("merge_base",) if "merge_base" in row else ())):
        if not re.fullmatch(r"[a-f0-9]{40}", row[key]):
            raise ValueError("Missing exact revision")
    return f"{repo.lower()}#{number}"


def split(repo):
    first = hashlib.sha256((SALT + repo.lower()).encode()).digest()[0]
    return "development" if first % 5 == 0 else "holdout"


def freeze(source, output, evaluation):
    rows = read(source)
    if not isinstance(rows, list) or not 1 <= len(rows) <= ROWS_COUNT_MAX:
        raise ValueError("Expected 1..2000 PRs")
    cohort, labels, keys = [], {}, set()
    for row in rows:
        key = identity(row)
        if key in keys:
            raise ValueError("Duplicate PR")
        keys.add(key)
        clean = {k: row[k] for k in FIELDS if k in row}
        clean.update(key=key, split=split(row["repo"]))
        cohort.append(clean)
        labels[key] = {k: row.get(k) for k in ("label", "why", "pair")}
    cohort.sort(key=lambda r: r["key"])
    result = {"version": 1, "salt": SALT, "source_sha256": hashlib.sha256(pathlib.Path(source).read_bytes()).hexdigest(), "rows": cohort}
    write(output, result)
    write(evaluation, {"version": 1, "cohort_sha256": digest(result), "labels": labels})
    return result


def git(repo, *args):
    result = subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "-C", str(repo), *args], capture_output=True, timeout=120, check=True)
    if len(result.stdout) > SOURCE_BYTES_MAX:
        raise ValueError("Source output exceeds 512 KiB")
    return result.stdout


def bundle(cohort, key, repo, output):
    row = next(r for r in read(cohort)["rows"] if r["key"] == key)
    base, head = row.get("merge_base", row["base_sha"]), row["head_sha"]
    if not re.fullmatch(r"[a-f0-9]{40}", base):
        raise ValueError("Invalid merge base")
    names = git(repo, "diff", "--no-renames", "--name-only", "-z", base, head).decode().split("\0")
    names = [n for n in names if n]
    if len(names) > FILES_COUNT_MAX:
        raise ValueError("More than 60 changed files; unsupported")
    snapshots, total = {}, 0
    for revision in (base, head):
        files = {}
        for name in names:
            try:
                data = git(repo, "show", f"{revision}:{name}")
            except subprocess.CalledProcessError:
                continue
            total += len(data)
            if total > SOURCE_BYTES_MAX:
                raise ValueError("Snapshots exceed 512 KiB; unsupported")
            files[name] = data.decode("utf-8", errors="replace")
        snapshots[revision] = files
    value = {"version": 1, "repository": row["repo"], "number": row["number"], "base": base, "head": head, "snapshots": snapshots}
    write(output, value)
    return value


def percentile(values, fraction):
    return sorted(values)[min(len(values) - 1, int(len(values) * fraction))] if values else None


def score(cohort, results, adjudications=None):
    frozen = read(cohort)
    rows = frozen["rows"]
    reviews = {}
    if adjudications:
        evaluation = read(adjudications)
        if evaluation.get("cohort_sha256") != digest(frozen):
            raise ValueError("Adjudication belongs to another frozen cohort or revision")
        reviews = evaluation["reviews"]
    records, invalid = {}, []
    for path in sorted(pathlib.Path(results).glob("*/result.json"))[:ROWS_COUNT_MAX]:
        try:
            value = read(path)
            key = f"{value['repository'].lower()}#{value['number']}"
            if key in records:
                raise ValueError("Duplicate result")
            expected = next(r for r in rows if r["key"] == key)
            if value["head"] != expected["head_sha"] or value["base"] != expected.get("merge_base", expected["base_sha"]):
                raise ValueError("Result revisions differ from the frozen PR")
            arms = value["results"]
            names = [a["arm"] for a in arms]
            if len(names) != len(set(names)) or not set(names) <= {"baseline", "hunter"}:
                raise ValueError("Duplicate or unknown arm")
            for arm in arms:
                seconds = arm["seconds"]
                if type(seconds) not in (int, float) or not math.isfinite(seconds) or seconds < 0:
                    raise ValueError("Invalid duration")
                if seconds > 1800 and any(j.get("confirmed") is True for j in arm["judgments"]):
                    raise ValueError("Over-budget confirmation")
            records[key] = value
        except (ValueError, KeyError, StopIteration) as error:
            invalid.append({"path": str(path), "why": str(error)})
    measured = []
    for row in rows:
        record = records.get(row["key"], {})
        core = record.get("core", {})
        core_review_items = sum(len(core[k]) for k in ("blocked", "look", "unverified")) if all(isinstance(core.get(k), list) for k in ("blocked", "look", "unverified")) else None
        for arm in ("baseline", "hunter"):
            result = next((r for r in record.get("results", []) if r.get("arm") == arm), None)
            review = reviews.get(row["key"], {}).get(arm, {})
            judgments = result.get("judgments", []) if result else []
            confirmed = [j for j in judgments if j.get("confirmed") is True]
            missing = sum(j.get("old") != ["Passed", "Passed"] or any(o == "Missing" for o in j.get("new", [])) or j.get("uneven") is True for j in judgments)
            over_budget = result is not None and (result.get("seconds", 1801) > 1800 or result.get("within_budget") is False)
            review = review if result else {}
            measured.append({"key": row["key"], "split": row["split"], "arm": arm,
                "status": "measured" if result else "not_run", "confirmed_candidates": len(confirmed),
                "confirmed_violations": review.get("confirmed_violations"), "wrong_blocks": review.get("wrong_blocks"),
                "review_items": core_review_items + len(confirmed) if result and core_review_items is not None else None,
                "core_review_items": core_review_items if result else None, "probe_review_items": len(confirmed) if result else None, "review_minutes": review.get("minutes"),
                "unsupported_probes": missing if result else None, "unsupported_pr": result is None or not judgments or missing == len(judgments) or over_budget,
                "seconds": result.get("seconds") if result else None,
                "fetch_seconds": record.get("fetch_seconds"), "model_seconds": record.get("model_seconds"), "end_to_end_seconds": record.get("end_to_end_seconds"),
                "model_cost_usd": record.get("cost_usd") if arm == "hunter" else 0,
                "infrastructure_cost_usd": None, "errors": result.get("errors", []) if result else [record.get("unsupported", "No execution artifact")]})
    summary = {"selected_prs": len(rows), "repositories": len({r["repo"].lower() for r in rows}), "invalid_artifacts": invalid}
    for arm in ("baseline", "hunter"):
        group = [r for r in measured if r["arm"] == arm]
        seconds = [r["seconds"] for r in group if isinstance(r["seconds"], (int, float))]
        summary[arm] = {"denominator": len(group), "run_prs": sum(r["status"] == "measured" for r in group),
            "unsupported_prs": sum(r["unsupported_pr"] for r in group), "confirmed_candidates": sum(r["confirmed_candidates"] for r in group),
            "adjudicated_prs": sum(r["confirmed_violations"] is not None and r["wrong_blocks"] is not None for r in group),
            "median_seconds": statistics.median(seconds) if seconds else None, "p90_seconds": percentile(seconds, .9),
            "cost_known_prs": sum(r["model_cost_usd"] is not None for r in group)}
    summary["by_split"] = {}
    for split_name in ("development", "holdout"):
        group = [r for r in measured if r["split"] == split_name]
        summary["by_split"][split_name] = {arm: {
            "denominator": sum(r["arm"] == arm for r in group),
            "run_prs": sum(r["arm"] == arm and r["status"] == "measured" for r in group),
            "unsupported_prs": sum(r["arm"] == arm and r["unsupported_pr"] for r in group),
        } for arm in ("baseline", "hunter")}
    paired = []
    for row in rows:
        a, b = [next(r for r in measured if r["key"] == row["key"] and r["arm"] == arm) for arm in ("baseline", "hunter")]
        if a["status"] == b["status"] == "measured":
            paired.append({"key": row["key"], "split": row["split"], "baseline_candidates": a["confirmed_candidates"], "hunter_candidates": b["confirmed_candidates"],
                "independently_adjudicated": all(r["confirmed_violations"] is not None and r["wrong_blocks"] is not None for r in (a, b))})
    summary["paired_prs"] = len(paired)
    summary["eligible_paired_prs"] = sum(all(not r["unsupported_pr"] for r in measured if r["key"] == p["key"]) for p in paired)
    return {"version": 1, "cohort_sha256": digest(frozen), "summary": summary, "rows": measured,
        "paired": paired,
        "claim": "No superiority claim is supported until both arms and independent adjudication are complete."}


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    for name, fields in (("freeze", ("source", "output", "evaluation")), ("bundle", ("cohort", "key", "repo", "output")), ("score", ("cohort", "results", "output"))):
        command = commands.add_parser(name)
        for field in fields:
            command.add_argument(field)
        if name == "score":
            command.add_argument("--adjudications")
    args = parser.parse_args()
    if args.command == "freeze":
        result = freeze(args.source, args.output, args.evaluation)
        print(json.dumps({"selected": len(result["rows"]), "cohort_sha256": digest(result)}))
    elif args.command == "bundle":
        bundle(args.cohort, args.key, args.repo, args.output)
    else:
        result = score(args.cohort, args.results, args.adjudications)
        write(args.output, result)
        print(json.dumps(result["summary"]))


if __name__ == "__main__":
    main()
