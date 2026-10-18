import hashlib
import json
import os
import pathlib
import subprocess
import sys
import shutil
import tempfile
import time
from benchmark import canonical, digest, read, write

CONFIGURATION = read(pathlib.Path(__file__).with_name("configuration.json"))
MODEL = CONFIGURATION["model"]
PROMPT_BYTES_MAX = CONFIGURATION["prompt_bytes_max"]
RESPONSE_BYTES_MAX = CONFIGURATION["response_bytes_max"]
OUTPUT_TOKENS_MAX = CONFIGURATION["output_tokens_max"]
CALL_SECS_MAX = CONFIGURATION["call_seconds_max"]
CALLS_COUNT_MAX = CONFIGURATION["calls_count_max"]
CLI_SHA256 = CONFIGURATION["cli_sha256"]


class ConfigurationError(ValueError):
    pass


def request_body(pack, bundle):
    expected = {"version", "repository", "number", "base", "head", "snapshots"}
    if set(bundle) != expected or bundle["version"] != 1:
        raise ValueError("Unexpected hunter context fields")
    for field in ("repository", "number", "base", "head"):
        if pack[field] != bundle[field]:
            raise ValueError("The hunter context names another PR or revision")
    if set(bundle["snapshots"]) != {pack["base"], pack["head"]}:
        raise ValueError("The hunter context contains another revision")
    instruction_bytes = pathlib.Path(__file__).with_name("HUNTER.md").read_bytes()
    if hashlib.sha256(instruction_bytes).hexdigest() != CONFIGURATION["instructions_sha256"]:
        raise ConfigurationError("The frozen hunter instructions changed")
    instructions = instruction_bytes.decode("utf-8")
    body = {"model": MODEL, "max_tokens": OUTPUT_TOKENS_MAX, "tools": [], "system": instructions,
        "messages": [{"role": "user", "content": canonical({"contracts": pack, "source": bundle}).decode()}]}
    if len(canonical(body)) > PROMPT_BYTES_MAX:
        raise ValueError("The complete prompt exceeds 64 KiB; do not silently truncate")
    if pack["contracts"] and all(c.get("input_domain", {}).get("type") == "null" for c in pack["contracts"]):
        raise ValueError("No additional approved input space; invoking the hunter would not test its value")
    return body


def decode(pack, body, response):
    if response.get("model") != MODEL or response.get("stop_reason") != "end_turn":
        raise ConfigurationError("Different model, incomplete response or refusal")
    blocks = response.get("content", [])
    if not blocks or any(b.get("type") != "text" for b in blocks):
        raise ValueError("Unexpected response block")
    answer = json.loads("".join(b["text"] for b in blocks))
    if set(answer) != {"proposals"}:
        raise ValueError("The model supplied fields outside its authority")
    groups = answer["proposals"]
    ids = {c["id"] for c in pack["contracts"]}
    if not isinstance(groups, list) or len(groups) > 4:
        raise ValueError("Too many proposal groups")
    seen = set()
    for group in groups:
        if set(group) != {"contract", "inputs"} or group["contract"] not in ids or group["contract"] in seen:
            raise ValueError("Unknown, duplicate or modified contract")
        seen.add(group["contract"])
        inputs = group["inputs"]
        if not isinstance(inputs, list) or not 1 <= len(inputs) <= 8 or any(len(canonical(i)) > 4096 for i in inputs):
            raise ValueError("Inputs exceed the fixed budget")
    usage = response.get("usage", {})
    count = lambda k: usage[k] if type(usage.get(k)) is int and usage[k] >= 0 else None
    return {"version": 1, "pack_sha256": digest(pack), "model": MODEL, "prompt_sha256": digest(body),
        "input_tokens": count("input_tokens"), "output_tokens": count("output_tokens"), "cost_usd": None, "proposals": groups}


def cli_infer(body):
    script = pathlib.Path(os.environ.get("APPDATA", "")) / "npm/node_modules/@anthropic-ai/claude-code/cli.js"
    if not script.is_file() or hashlib.sha256(script.read_bytes()).hexdigest() != CLI_SHA256:
        raise ConfigurationError("The audited Claude Code 2.0.55 client is absent or changed; no fallback")
    node = shutil.which("node")
    if not node:
        raise ValueError("The audited CLI needs the installed Node runtime")
    command = [node, str(script), "--print", "--output-format", "json", "--model", MODEL, "--max-turns", "1", "--max-thinking-tokens", "0", "--max-budget-usd", "0.5",
        "--tools", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}', "--setting-sources", "", "--system-prompt", body["system"]]
    env = dict(os.environ, CLAUDE_CODE_DISABLE_CLAUDE_MDS="1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1", CLAUDE_CODE_MAX_OUTPUT_TOKENS=str(OUTPUT_TOKENS_MAX), CLAUDE_CODE_MAX_RETRIES="0")
    with tempfile.TemporaryDirectory(prefix="kern-hunter-") as cwd:
        env["GIT_CEILING_DIRECTORIES"] = cwd
        result = subprocess.run(command, input=body["messages"][0]["content"], text=True, encoding="utf-8", stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            cwd=cwd, env=env, timeout=CALL_SECS_MAX)
    if result.returncode != 0 or len(result.stdout.encode()) > RESPONSE_BYTES_MAX:
        raise ValueError("The isolated CLI failed or exceeded its output budget")
    value = json.loads(result.stdout)
    if value.get("is_error") is not False or value.get("num_turns") != 1 or set(value.get("modelUsage", {})) != {MODEL}:
        raise ConfigurationError("The CLI used an unexpected model, extra turn or error path; stop this configuration")
    cost = value.get("total_cost_usd")
    if isinstance(cost, (int, float)) and cost > .5:
        raise ConfigurationError("The CLI exceeded its per-call cost ceiling; stop this configuration")
    return {"model": MODEL, "stop_reason": "end_turn", "content": [{"type": "text", "text": value["result"]}], "usage": value.get("usage", {}),
        "cli_sha256": CLI_SHA256, "client": "claude-code-2.0.55", "estimated_cost_usd": value.get("total_cost_usd")}


def run(cohort, bundles, packs, output, backend="cli"):
    if backend != CONFIGURATION["backend"]:
        raise ConfigurationError("This pilot has one frozen backend; a different one requires a new configuration")
    rows = read(cohort)["rows"]
    if not 1 <= len(rows) <= CALLS_COUNT_MAX:
        raise ValueError("This fixed pilot permits at most six PRs")
    output = pathlib.Path(output)
    output.mkdir()
    stopped = False
    for row in rows:
        name = row["repo"].replace("/", "__") + "__" + str(row["number"])
        case = output / name
        case.mkdir()
        start = time.monotonic()
        response = None
        try:
            if stopped:
                raise ConfigurationError("Configuration stopped after an earlier invalid response")
            pack, bundle = read(pathlib.Path(packs) / f"{name}.json"), read(pathlib.Path(bundles) / f"{name}.json")
            body = request_body(pack, bundle)
            write(case / "request.json", body)
            response = cli_infer(body)
            write(case / "provider-response.json", response)
            if time.monotonic() - start > CALL_SECS_MAX:
                raise ValueError("The model call exceeded its budget; response is retained but does not count")
            proposal = decode(pack, body, response)
            proposal["cost_usd"] = response.get("estimated_cost_usd")
            proposal["model"] = f"{MODEL}/claude-code-2.0.55/{CLI_SHA256[:12]}"
            write(case / "proposals.json", proposal)
            status = {"status": "proposed", "model": MODEL, "usage": response.get("usage"), "cost_usd": None}
        except Exception as error:
            if isinstance(error, ConfigurationError):
                stopped = True
            status = {"status": "unsupported", "error_type": type(error).__name__, "why": str(error)[:400]}
        status["usage"] = response.get("usage") if isinstance(response, dict) else None
        status["cost_usd"] = None
        status["backend"] = backend
        status["seconds"] = time.monotonic() - start
        write(case / "status.json", status)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--cli"]:
        run(*sys.argv[2:], backend="cli")
    else:
        run(*sys.argv[1:])
