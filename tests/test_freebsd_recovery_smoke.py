"""Execute the recovery workflow's actual embedded Python, never a copied harness."""
import ast
import os
from pathlib import Path
import re
import socket
import stat
import subprocess
import tempfile
import textwrap
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/freebsd-0.2.1-recovery.yml"
SOURCE = textwrap.dedent(WORKFLOW.read_text().split("python3 - <<'PY'\n", 1)[1].split("            PY\n", 1)[0])
TREE = ast.parse(SOURCE)


def functions():
    nodes = [node for node in TREE.body if isinstance(node, (ast.Import, ast.FunctionDef))]
    namespace = {}
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(WORKFLOW), "exec"), namespace)
    return namespace


class RecoverySmokeTests(unittest.TestCase):
    def test_runtime_is_private_short_and_binds_actual_host_socket(self):
        host = (ROOT / "src/host/instance.rs").read_text()
        filename = re.search(r'None => "([^"]+\.sock)"\.to_owned\(\)', host).group(1)
        with tempfile.TemporaryDirectory(prefix="long-checkout-" + "x" * 100) as checkout:
            with patch.dict(os.environ, {"TMPDIR": checkout}):
                runtime = functions()["private_runtime"]()
            path = Path(runtime.name)
            try:
                self.assertEqual(path.parent, Path("/tmp"))
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o700)
                address = str(path / filename)
                # FreeBSD sockaddr_un.sun_path is 104 bytes, including NUL.
                self.assertLess(len(os.fsencode(address)), 104)
                with socket.socket(socket.AF_UNIX) as listener:
                    listener.bind(address)
                    listener.listen(1)
            finally:
                runtime.cleanup()
            self.assertFalse(path.exists())

    def run_gate(self, tree, polls):
        namespace = functions()
        app = Mock()
        app.poll.side_effect = polls
        with tempfile.TemporaryDirectory() as directory:
            smoke = Path(directory)
            with patch.object(namespace["subprocess"], "check_output", return_value=tree) as output, patch.object(namespace["time"], "sleep") as sleep:
                namespace["check_native_window"](app, {}, smoke)
                self.assertEqual(output.call_args.args[0], ["xwininfo", "-root", "-tree"])
                self.assertNotIn("text", output.call_args.kwargs)
                sleep.assert_called_once_with(8)
                return (smoke / "windows.log").read_text()

    def test_malformed_other_window_title_keeps_exact_ascii_jcode_gate(self):
        log = self.run_gate(b'0x1 "bad\xe0title"\n0x2 "Jcode" ("jcode" "Jcode")\n', [None, None])
        self.assertIn("bad\ufffdtitle", log)
        self.assertIn('"Jcode"', log)

    def test_process_exit_before_window_rejected(self):
        with self.assertRaisesRegex(AssertionError, "before creating"):
            self.run_gate(b'0x2 "Jcode" ("jcode" "Jcode")', [1])

    def test_process_exit_during_eight_second_gate_rejected(self):
        with self.assertRaisesRegex(AssertionError, "during graphical smoke"):
            self.run_gate(b'0x2 "Jcode" ("jcode" "Jcode")', [None, 1])

    def test_nonexact_or_malformed_jcode_title_rejected(self):
        for title in (b'0x2 "Jcode impostor":', b'0x2 "Jcod\xe0":', b'0x2 "NotJcode":', b'0x2 "NotJcode" ("jcode" "Jcode")'):
            with self.subTest(title=title), self.assertRaisesRegex(AssertionError, "did not create"):
                self.run_gate(title, [None] * 100)

    def test_xwininfo_failure_rejected(self):
        namespace = functions()
        with patch.object(namespace["subprocess"], "check_output", side_effect=subprocess.CalledProcessError(1, "xwininfo")):
            with self.assertRaises(subprocess.CalledProcessError):
                namespace["check_native_window"](Mock(poll=Mock(return_value=None)), {}, Path("unused"))

    def test_cleanup_waits_for_processes_before_runtime_removal_and_retains_logs(self):
        cleanup = next(node.finalbody for node in TREE.body if isinstance(node, ast.Try))
        events = []
        process = Mock()
        process.poll.return_value = None
        process.terminate.side_effect = lambda: events.append("terminate")
        process.wait.side_effect = [subprocess.TimeoutExpired("app", 5), None]
        process.kill.side_effect = lambda: events.append("kill")
        with tempfile.TemporaryDirectory() as directory:
            smoke = Path(directory)
            runtime = functions()["private_runtime"]()
            runtime_path = Path(runtime.name)
            original_cleanup = runtime.cleanup
            def clean():
                self.assertEqual(process.wait.call_count, 2)
                events.append("cleanup")
                original_cleanup()
            runtime.cleanup = clean
            log = (smoke / "app.log").open("w")
            log.write("retained failure diagnostics")
            namespace = dict(processes=[process], runtime=runtime, logs=[log], smoke=smoke, subprocess=subprocess)
            with patch("builtins.print"):
                exec(compile(ast.Module(body=cleanup, type_ignores=[]), str(WORKFLOW), "exec"), namespace)
            self.assertEqual(events, ["terminate", "kill", "cleanup"])
            self.assertFalse(runtime_path.exists())
            self.assertTrue(log.closed)
            self.assertEqual((smoke / "app.log").read_text(), "retained failure diagnostics")


if __name__ == "__main__":
    unittest.main()
