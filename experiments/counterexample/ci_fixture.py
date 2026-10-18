import json
import pathlib
import subprocess
import sys
from benchmark import digest, write


def git(root, *args):
    return subprocess.run(["git", "-c", "core.hooksPath=/dev/null", "-C", str(root), *args], check=True, capture_output=True, text=True, timeout=30).stdout.strip()


def prepare(root):
    root.mkdir()
    git(root, "init", "-q")
    git(root, "config", "user.name", "Validation fixture")
    git(root, "config", "user.email", "fixture@example.invalid")
    package = {"name": "validation-fixture", "version": "1.0.0", "private": True, "type": "module", "scripts": {"test": "node --test", "build": "node --check guard.js"}}
    (root / "package.json").write_text(json.dumps(package))
    (root / "package-lock.json").write_text(json.dumps({"name": "validation-fixture", "version": "1.0.0", "lockfileVersion": 3, "packages": {"": package}}))
    (root / "SECURITY.md").write_text("The allowed function must return false for every path beginning with ../.\n")
    (root / "guard.js").write_text("export const allowed = path => !path.startsWith('../');\n")
    (root / "guard.test.js").write_text("import {test} from 'node:test';import assert from 'node:assert/strict';import {allowed} from './guard.js';test('denies parent',()=>assert.equal(allowed('../secret'),false));\n")
    git(root, "add", "-A")
    git(root, "commit", "-qm", "fixture base")
    base = git(root, "rev-parse", "HEAD")
    (root / "guard.js").write_text("export const allowed = path => path !== '../secret';\n")
    git(root, "commit", "-qam", "fixture incomplete rejection")
    broken = git(root, "rev-parse", "HEAD")
    git(root, "checkout", "-q", "--detach", base)
    (root / "guard.js").write_text("export const allowed = path => !path.startsWith('../');\n\n")
    git(root, "commit", "-qam", "fixture equivalent edit")
    return base, broken, git(root, "rev-parse", "HEAD")


def artifacts(root, base, head, number):
    source = root / "source"
    security = source / "SECURITY.md"
    import hashlib
    contracts = [{"id": "deny-parent", "kind": "security", "source": "SECURITY.md", "source_sha256": hashlib.sha256(security.read_bytes()).hexdigest(),
        "input_domain": {"description": "Every string starting with ../ must be denied", "type": "string-prefix", "prefix": "../"}, "command": ["/usr/local/bin/node", "--input-type=module", "-e", "const {allowed}=await import('./guard.js');process.stdout.write(JSON.stringify(allowed(JSON.parse(process.argv[1]))));"],
        "oracle": {"mode": "json-equal", "expected": False}, "baseline_inputs": ["../secret"]}]
    pack = {"version": 1, "repository": "kern/validation-fixture", "number": number, "base": base, "head": head, "approved_by": "trusted synthetic fixture; not a real PR", "contracts": contracts}
    proposal = {"version": 1, "pack_sha256": digest(pack), "model": "structural-fixture-no-model", "prompt_sha256": "a" * 64,
        "input_tokens": None, "output_tokens": None, "cost_usd": None, "proposals": [{"contract": "deny-parent", "inputs": ["../../secret"]}]}
    write(root / f"pack-{number}.json", pack)
    write(root / f"proposal-{number}.json", proposal)


def main():
    root, worker, operator, control = map(pathlib.Path, sys.argv[1:])
    root.mkdir()
    base, broken, equivalent = prepare(root / "source")
    for number, head in enumerate((broken, equivalent), 1):
        print(f"Starting structural fixture {number}: {'regression' if number == 1 else 'equivalent edit'}", flush=True)
        artifacts(root, base, head, number)
        subprocess.run([str(worker), str(root / f"pack-{number}.json"), str(root / f"proposal-{number}.json"), str(operator), str(control), str(root / "source"), str(root / f"result-{number}")], check=True, timeout=3600)
        report = json.loads((root / f"result-{number}" / "result.json").read_text())
        arms = {r["arm"]: r for r in report["results"]}
        expected = {"baseline": 0, "hunter": 1 if number == 1 else 0}
        for name, count in expected.items():
            actual = sum(j["confirmed"] for j in arms[name]["judgments"])
            if actual != count or not arms[name]["within_budget"] or arms[name]["errors"]:
                raise SystemExit(f"Synthetic fixture {number}/{name}: expected {count}, got {actual}; inspect raw artifacts")
        print(f"Structural fixture {number} passed", flush=True)
    print("Synthetic structural fixtures passed; these do not establish real-PR or AI performance.")


if __name__ == "__main__":
    main()
