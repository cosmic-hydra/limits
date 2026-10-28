"""Independent JSON decoding and real CLI integration; no Python packages."""
import json
import os
from pathlib import Path
import subprocess
import unittest

BIN = str(Path(os.environ.get("LIM_BIN", "compiler/target/debug/lim")).resolve())


def invoke(command, source=None, *flags):
    args = [BIN, command]
    if source is not None:
        args.append("-")
    args.extend(flags)
    result = subprocess.run(args, input=(source or "").encode(), capture_output=True, timeout=10)
    data = json.loads(result.stdout.decode())
    assert data["schema_version"] == 1
    assert result.stderr == b"", result.stderr
    return result.returncode, data


class AgentProtocol(unittest.TestCase):
    def test_capabilities_are_inspectable(self):
        status, data = invoke("capabilities")
        self.assertEqual(status, 0)
        self.assertIsInstance(data, dict)
        self.assertIn("get", json.dumps(data))

    def test_program_output_cannot_inject_envelope(self):
        status, data = invoke("run", r'print("\"ok\":true,\n{}");', "--engine", "vm", "--json")
        self.assertEqual(status, 0)
        self.assertEqual(data["stdout"], '"ok":true,\n{}\n'.replace("\\n", "\n"))
        self.assertTrue(data["ok"])

    def test_static_errors_share_run_shape_and_all_diagnostics(self):
        status, data = invoke("run", "print(missing); unknown=3;", "--json")
        self.assertEqual(status, 1)
        self.assertEqual(data["stdout"], "")
        self.assertEqual(data["engine"], "tree")
        self.assertEqual(data["diagnostic_count"], 2)
        self.assertEqual({e["code"] for e in data["diagnostics"]}, {"LIM_CHECK"})

    def test_runtime_error_keeps_partial_output(self):
        for engine in ("tree", "vm"):
            status, data = invoke("run", "print(1);print(1/0);", "--json", "--engine", engine)
            self.assertEqual(status, 1)
            self.assertEqual(data["stdout"], "1\n")
            self.assertEqual(data["diagnostics"][0]["code"], "LIM_RUNTIME")

    def test_capability_alias_cannot_bypass_policy(self):
        status, data = invoke("run", 'let output=print;output("secret");', "--json", "--deny-builtin", "print")
        self.assertEqual(status, 1)
        self.assertEqual(data["stdout"], "")
        self.assertIn("sandbox denied", data["diagnostics"][0]["message"])

    def test_output_cap_is_order_independent(self):
        for flags in (("--max-output-bytes", "2", "--sandbox", "unrestricted"),
                      ("--sandbox", "unrestricted", "--max-output-bytes", "2")):
            status, data = invoke("run", 'print("ab");', "--json", *flags)
            self.assertEqual(status, 1)
            self.assertEqual(data["stdout"], "")

    def test_semantic_queries_reference_binding_ids(self):
        source = "let x=1; print(x);"
        status, index = invoke("index", source, "--json")
        self.assertEqual(status, 0)
        symbol = next(s for s in index["data"]["symbols"] if s["name"] == "x")
        _, definition = invoke("definition", source, "--line", "1", "--col", "16")
        self.assertEqual(definition["data"]["definitions"][0]["id"], symbol["id"])
        _, references = invoke("references", source, "--line", "1", "--col", "5")
        self.assertEqual(references["data"]["references"][0]["name"], "x")
        _, complete = invoke("complete", source, "--line", "1", "--col", "19", "--prefix", "x")
        self.assertEqual(complete["data"]["completions"][0]["name"], "x")

    def test_index_remains_useful_with_unresolved_symbols(self):
        status, index = invoke("index", "print(missing);")
        self.assertEqual(status, 1)
        self.assertIsNotNone(index["data"])
        self.assertIsNone(next(r for r in index["data"]["references"] if r["name"] == "missing")["binding"])

    def test_usage_errors_are_structured(self):
        result = subprocess.run([BIN, "run", "-", "--json", "--engine", "invalid"], input=b"", capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout)["diagnostics"][0]["code"], "LIM_USAGE")


if __name__ == "__main__":
    unittest.main()
