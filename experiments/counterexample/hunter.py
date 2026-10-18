import hashlib
import json
import math
import os
import pathlib
import re
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
    def __init__(self, message, diagnostics=None):
        super().__init__(message)
        self.diagnostics = diagnostics


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


def client_measurements(value):
    models = value.get("modelUsage", {})
    names = sorted(models)[:16] if isinstance(models, dict) else []
    names = [n if re.fullmatch(r"claude-(sonnet|haiku|opus)-[0-9-]{1,40}", n) else "<unrecognized-model>" for n in names]
    usage = value.get("usage", {})
    counts = {k: usage[k] for k in ("input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")
        if isinstance(usage, dict) and type(usage.get(k)) is int and usage[k] >= 0}
    cost = value.get("total_cost_usd")
    cost = cost if type(cost) in (int, float) and math.isfinite(cost) and cost >= 0 else None
    turns = value.get("num_turns")
    return {"models": names, "num_turns": turns if type(turns) is int and turns >= 0 else None,
        "is_error": value.get("is_error") if type(value.get("is_error")) is bool else None,
        "usage": counts, "estimated_cost_usd": cost}


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
    if len(result.stdout.encode()) > RESPONSE_BYTES_MAX:
        raise ConfigurationError("The isolated CLI exceeded its output budget")
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise ConfigurationError("The CLI returned malformed result JSON") from None
    if not isinstance(value, dict):
        raise ConfigurationError("The CLI returned no result object")
    diagnostics = client_measurements(value)
    models = value.get("modelUsage")
    if result.returncode != 0 or value.get("is_error") is not False or type(value.get("num_turns")) is not int or value["num_turns"] != 1 or not isinstance(models, dict) or set(models) != {MODEL} or not isinstance(value.get("result"), str):
        raise ConfigurationError("The CLI used an unexpected model, extra turn or error path; stop this configuration", diagnostics)
    cost = diagnostics["estimated_cost_usd"]
    if value.get("total_cost_usd") is not None and (cost is None or cost > .5):
        raise ConfigurationError("The CLI returned invalid cost evidence or exceeded its estimated cost ceiling; stop this configuration", diagnostics)
    return {"model": MODEL, "stop_reason": "end_turn", "content": [{"type": "text", "text": value["result"]}], "usage": diagnostics["usage"],
        "cli_sha256": CLI_SHA256, "client": "claude-code-2.0.55", "estimated_cost_usd": cost}


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
            proposal["model"] = f"{MODEL}/claude-code-2.0.55/{CLI_SHA256[:12]}"
            write(case / "proposals.json", proposal)
            status = {"status": "proposed", "model": MODEL, "usage": response.get("usage"), "cost_usd": None}
        except Exception as error:
            if isinstance(error, ConfigurationError):
                stopped = True
            status = {"status": "unsupported", "error_type": type(error).__name__, "why": str(error)[:400]}
            if isinstance(error, ConfigurationError) and error.diagnostics is not None:
                status["client_diagnostics"] = error.diagnostics
                write(case / "client-diagnostics.json", error.diagnostics)
        status["usage"] = response.get("usage") if isinstance(response, dict) else status.get("client_diagnostics", {}).get("usage")
        status["estimated_cost_usd"] = response.get("estimated_cost_usd") if isinstance(response, dict) else status.get("client_diagnostics", {}).get("estimated_cost_usd")
        status["cost_usd"] = None
        status["backend"] = backend
        status["seconds"] = time.monotonic() - start
        write(case / "status.json", status)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--cli"]:
        run(*sys.argv[2:], backend="cli")
    else:
        run(*sys.argv[1:])
