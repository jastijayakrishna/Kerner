import copy
import json
import hashlib
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from hunter import CALL_SECS_MAX, MODEL, ConfigurationError, cli_infer, decode, request_body, run


class HunterTests(unittest.TestCase):
    def setUp(self):
        self.pack = {"version": 1, "repository": "example/project", "number": 1, "base": "1" * 40, "head": "2" * 40, "contracts": [{"id": "allowed-contract"}]}
        self.bundle = {"version": 1, "repository": "example/project", "number": 1, "base": "1" * 40, "head": "2" * 40, "snapshots": {"1" * 40: {}, "2" * 40: {}}}
        self.body = request_body(self.pack, self.bundle)
        self.response = {"model": MODEL, "stop_reason": "end_turn", "content": [{"type": "text", "text": '{"proposals":[]}'}], "usage": {"input_tokens": 100, "output_tokens": 10}}

    def test_request_has_no_tools_history_or_evaluation_context(self):
        self.assertEqual(self.body["tools"], [])
        self.assertEqual(len(self.body["messages"]), 1)
        self.assertEqual(self.body["model"], MODEL)
        bad = dict(self.bundle, labels={"future": "fix"})
        with self.assertRaises(ValueError):
            request_body(self.pack, bad)
        bad = copy.deepcopy(self.bundle)
        bad["snapshots"]["3" * 40] = {}
        with self.assertRaises(ValueError):
            request_body(self.pack, bad)

    def test_empty_response_is_valid_but_never_fabricates_billed_cost(self):
        result = decode(self.pack, self.body, self.response)
        self.assertEqual(result["proposals"], [])
        self.assertEqual(result["input_tokens"], 100)
        self.assertIsNone(result["cost_usd"])

    def test_model_drift_truncation_and_oracle_replacement_are_rejected(self):
        for field, value in (("model", "another-model"), ("stop_reason", "max_tokens")):
            bad = dict(self.response, **{field: value})
            with self.assertRaises(ValueError):
                decode(self.pack, self.body, bad)
        bad = copy.deepcopy(self.response)
        bad["content"][0]["text"] = json.dumps({"proposals": [{"contract": "allowed-contract", "inputs": [1], "oracle": "always pass"}]})
        with self.assertRaises(ValueError):
            decode(self.pack, self.body, bad)

    def test_prompt_budget_cannot_be_bypassed_by_silent_truncation(self):
        bad = copy.deepcopy(self.bundle)
        bad["snapshots"]["1" * 40]["source.js"] = "x" * (64 << 10)
        with self.assertRaises(ValueError):
            request_body(self.pack, bad)

    def test_a_null_only_contract_cannot_be_billed_as_an_ai_hunting_experiment(self):
        pack = copy.deepcopy(self.pack)
        pack["contracts"][0]["input_domain"] = {"type": "null", "description": "No variable inputs"}
        with self.assertRaises(ValueError):
            request_body(pack, self.bundle)

    def test_cli_disables_memory_tools_settings_and_reuses_no_session(self):
        with tempfile.TemporaryDirectory() as root:
            script = pathlib.Path(root) / "npm/node_modules/@anthropic-ai/claude-code/cli.js"
            script.parent.mkdir(parents=True)
            script.write_text("inert fixture; never executed")
            reply = {"is_error": False, "num_turns": 1, "modelUsage": {MODEL: {}}, "result": '{"proposals":[]}', "total_cost_usd": .01}
            with patch("hunter.os.environ", {"APPDATA": root}), patch("hunter.CLI_SHA256", hashlib.sha256(script.read_bytes()).hexdigest()), patch("hunter.shutil.which", return_value="trusted-node"):
                with patch("hunter.subprocess.run", return_value=subprocess.CompletedProcess([], 0, json.dumps(reply))) as call:
                    result = cli_infer(self.body)
                    args, options = call.call_args.args[0], call.call_args.kwargs
                    self.assertEqual(args[args.index("--tools") + 1], "")
                    self.assertEqual(args[args.index("--setting-sources") + 1], "")
                    self.assertNotIn("--resume", args)
                    self.assertNotIn("--continue", args)
                    self.assertEqual(options["env"]["CLAUDE_CODE_DISABLE_CLAUDE_MDS"], "1")
                    self.assertEqual(options["timeout"], CALL_SECS_MAX)
                    self.assertEqual(result["estimated_cost_usd"], .01)
                with patch("hunter.subprocess.run", side_effect=subprocess.TimeoutExpired("request", CALL_SECS_MAX)) as call:
                    with self.assertRaises(subprocess.TimeoutExpired):
                        cli_infer(self.body)
                    self.assertEqual(call.call_count, 1)
                    self.assertEqual(call.call_args.kwargs["timeout"], CALL_SECS_MAX)

    def test_rejected_client_response_keeps_measurements_without_response_text(self):
        with tempfile.TemporaryDirectory() as root:
            script = pathlib.Path(root) / "npm/node_modules/@anthropic-ai/claude-code/cli.js"
            script.parent.mkdir(parents=True)
            script.write_text("inert fixture; never executed")
            reply = {"is_error": False, "num_turns": 1, "modelUsage": {MODEL: {}, "claude-haiku-4-5-20251001": {}},
                "result": "DO_NOT_RETAIN_RESPONSE_TEXT", "total_cost_usd": .03, "usage": {"input_tokens": 10, "output_tokens": 5}}
            with patch("hunter.os.environ", {"APPDATA": root}), patch("hunter.CLI_SHA256", hashlib.sha256(script.read_bytes()).hexdigest()), patch("hunter.shutil.which", return_value="trusted-node"):
                with patch("hunter.subprocess.run", return_value=subprocess.CompletedProcess([], 0, json.dumps(reply))):
                    with self.assertRaises(ConfigurationError) as raised:
                        cli_infer(self.body)
            metadata = raised.exception.diagnostics
            self.assertEqual(metadata["usage"], reply["usage"])
            self.assertEqual(metadata["estimated_cost_usd"], .03)
            self.assertIn("claude-haiku-4-5-20251001", metadata["models"])
            self.assertNotIn("DO_NOT_RETAIN_RESPONSE_TEXT", json.dumps(metadata))

    def test_client_estimate_never_becomes_a_claim_about_billed_cost(self):
        with tempfile.TemporaryDirectory() as root:
            root = pathlib.Path(root)
            for folder, value in (("packs", self.pack), ("bundles", self.bundle)):
                (root / folder).mkdir()
                (root / folder / "example__project__1.json").write_text(json.dumps(value))
            cohort = root / "cohort.json"
            cohort.write_text(json.dumps({"rows": [{"repo": "example/project", "number": 1}]}))
            response = dict(self.response, estimated_cost_usd=.03)
            with patch("hunter.cli_infer", return_value=response):
                run(cohort, root / "bundles", root / "packs", root / "output")
            case = root / "output/example__project__1"
            self.assertIsNone(json.loads((case / "proposals.json").read_text())["cost_usd"])
            self.assertEqual(json.loads((case / "provider-response.json").read_text())["estimated_cost_usd"], .03)


if __name__ == "__main__":
    unittest.main()
