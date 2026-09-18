"""Offline tests: python3 -m unittest discover -s scripts/cloud_alpha -p test_idle.py."""

import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import idle


def empty():
    return {"errors": [], "ssh": False, "work_pids": [], "jcode_pids": [], "tasks": []}


def row(uid=1000, ppid=1, name="bash", argv=None, state="S"):
    return {"uid": uid, "ppid": ppid, "name": name,
            "argv": [name] if argv is None else argv, "state": state}


class PolicyTests(unittest.TestCase):
    def test_positive_idle_requires_complete_inventory(self):
        self.assertTrue(idle.policy(empty())["idle"])
        self.assertFalse(idle.policy({})["idle"])

    def test_no_ssh_is_not_proof_of_idle(self):
        for key, value in (("work_pids", [20]), ("jcode_pids", [21]),
                           ("errors", ["unknown"]), ("ssh", True)):
            with self.subTest(key=key):
                observation = empty()
                observation[key] = value
                self.assertFalse(idle.policy(observation)["idle"])

    def test_daemon_with_synthetic_idle_listing_never_proves_idle(self):
        for sessions in ([], [{"status": "idle"}], [{"status": "attached"}],
                         [{"status": "running"}], [{"status": "future_status"}]):
            observation = empty()
            observation.update(jcode_pids=[10], api={"sessions": sessions})
            self.assertFalse(idle.policy(observation)["idle"])

    def test_task_status_exact_terminal_allowlist(self):
        for status in ("running", "queued", None, "idle", "cancelled", "Completed"):
            observation = empty()
            observation["tasks"] = [{"status": status, "pid_alive": False}]
            self.assertFalse(idle.policy(observation)["idle"], status)
        for status in ("completed", "failed", "superseded"):
            observation = empty()
            observation["tasks"] = [{"status": status, "pid_alive": False}]
            self.assertTrue(idle.policy(observation)["idle"], status)

    def test_running_record_without_pid_or_old_timestamp_is_busy(self):
        observation = empty()
        observation["tasks"] = [{"status": "running", "pid": None,
                                 "started_at": "2000-01-01", "pid_alive": False}]
        self.assertFalse(idle.policy(observation)["idle"])

    def test_terminal_record_with_live_pid_is_busy(self):
        observation = empty()
        observation["tasks"] = [{"status": "completed", "pid_alive": True}]
        self.assertFalse(idle.policy(observation)["idle"])

    def test_malformed_task_inventory_is_not_idle(self):
        for tasks in (None, {}, [None], [{}]):
            observation = empty()
            observation["tasks"] = tasks
            self.assertFalse(idle.policy(observation)["idle"])

    def test_unknown_field_types_fail_safe(self):
        for field in ("ssh", "work_pids", "jcode_pids", "errors", "tasks"):
            observation = empty()
            observation[field] = None
            self.assertFalse(idle.policy(observation)["idle"], field)
        self.assertFalse(idle.policy(None)["idle"])
        observation = empty()
        observation["tasks"] = [{"status": ["completed"]}]
        self.assertFalse(idle.policy(observation)["idle"])


class ClockTests(unittest.TestCase):
    def test_requires_30_minutes_of_continuous_idle_samples(self):
        previous = None
        for now in range(100, 1900, 60):
            previous, stop, elapsed = idle.advance(previous, {"idle": True}, "boot", now)
            self.assertFalse(stop)
        previous, stop, elapsed = idle.advance(previous, {"idle": True}, "boot", 1900)
        self.assertTrue(stop)
        self.assertEqual(elapsed, 1800)

    def test_busy_and_unknown_reset_entire_grace(self):
        previous = {"boot_id": "boot", "checked_at": 1800, "idle_since": 0}
        state, stop, elapsed = idle.advance(previous, {"idle": False}, "boot", 1801)
        self.assertFalse(stop)
        self.assertIsNone(state["idle_since"])
        _, stop, elapsed = idle.advance(state, {"idle": True}, "boot", 1860)
        self.assertFalse(stop)
        self.assertEqual(elapsed, 0)

    def test_reboot_clock_reversal_gap_and_corrupt_state_reset(self):
        base = {"boot_id": "boot", "checked_at": 1800, "idle_since": 0}
        cases = [(base, "new_boot", 1801), (base, "boot", 1799),
                 (base, "boot", 1800 + idle.MAX_GAP + 1),
                 ({**base, "idle_since": "0"}, "boot", 1801),
                 ({**base, "idle_since": float("nan")}, "boot", 1801),
                 ({**base, "checked_at": float("inf")}, "boot", 1801),
                 ({**base, "idle_since": -5}, "boot", 1801),
                 ({**base, "idle_since": True}, "boot", 1801),
                 ([], "boot", 1801)]
        for previous, boot, now in cases:
            with self.subTest(previous=previous, boot=boot, now=now):
                _, stop, elapsed = idle.advance(previous, {"idle": True}, boot, now)
                self.assertFalse(stop)
                self.assertEqual(elapsed, 0)


class ProcessTests(unittest.TestCase):
    def test_detached_user_work_and_privileged_descendants(self):
        rows = {1: row(0, 0, "systemd"), 10: row(1000, 1, "jcode"),
                11: row(0, 10, "sudo"), 12: row(0, 11, "cargo"),
                20: row(1000, 1, "sleep"), 30: row(0, 1, "sshd")}
        work, jcode = idle.work_processes(rows, 1000)
        self.assertEqual(work, [10, 11, 12, 20])
        self.assertEqual(jcode, [10])

    def test_root_jcode_also_blocks_and_zombies_do_not(self):
        rows = {1: row(0, 0, "systemd"), 3: row(0, 1, "jcode"),
                4: row(0, 3, "tool"), 8: row(1000, 1, "bash", state="Z")}
        self.assertEqual(idle.work_processes(rows, 1000), ([3, 4], [3]))

    def test_all_user_processes_including_unrecognized_daemons_are_conservative(self):
        rows = {3: row(1000, 1, "custom-daemon")}
        self.assertEqual(idle.work_processes(rows, 1000)[0], [3])

    def test_non_pty_ssh_and_nonstandard_port_are_active(self):
        self.assertTrue(idle.ssh_connected({5: row(0, 1, "sshd", ["sshd: ec2-user@notty"])}))

    def test_tcp_ipv4_ipv6_and_listener(self):
        with tempfile.TemporaryDirectory() as directory:
            proc = Path(directory)
            (proc / "net").mkdir()
            header = "sl local_address rem_address st\n"
            (proc / "net/tcp").write_text(header + "0: 00000000:0016 00000000:0000 0A\n")
            self.assertFalse(idle.ssh_connected({}, proc))
            (proc / "net/tcp6").write_text(header + "0: 0000000000000000:0016 0000000000000000:9876 01\n")
            self.assertTrue(idle.ssh_connected({}, proc))
            (proc / "net/tcp6").unlink()
            (proc / "net/tcp").write_text(header + "0: 00000000:0016 00000000:9876 01\n")
            self.assertTrue(idle.ssh_connected({}, proc))

    def test_inspection_errors_are_fail_safe(self):
        with patch.object(idle.pwd, "getpwnam", side_effect=KeyError("missing user")):
            result = idle.inspect()
        self.assertTrue(result["errors"])
        self.assertFalse(idle.policy(result)["idle"])

    def test_idle_inspection_does_not_spawn_api(self):
        with patch.object(idle.pwd, "getpwnam") as user, \
                patch.object(idle, "processes", return_value={}), \
                patch.object(idle, "ssh_connected", return_value=False), \
                patch.object(idle, "background_tasks", return_value=[]), \
                patch.object(idle, "api_sessions") as api:
            user.return_value.pw_uid = 1000
            self.assertTrue(idle.policy(idle.inspect())["idle"])
            api.assert_not_called()


class EntrypointTests(unittest.TestCase):
    def test_check_never_writes_state_or_shutdown_even_when_due(self):
        due = {"would_poweroff": True, "idle": True}
        with patch.object(idle, "evaluate", return_value=({}, due)), \
                patch.object(idle, "save_state") as save, \
                patch.object(idle.subprocess, "run") as run, \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(idle.main(["--check"]), 0)
        save.assert_not_called()
        run.assert_not_called()
        self.assertEqual(json.loads(output.getvalue())["action"], "check_only")

    def test_due_timer_rechecks_and_saves_before_shutdown(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(idle, "STATE_DIR", Path(directory)), \
                patch.object(idle.os, "geteuid", return_value=0), \
                patch.object(idle.os, "umask"), \
                patch.object(idle, "evaluate", return_value=({"idle_since": 0}, {"would_poweroff": True})), \
                patch.object(idle.subprocess, "run") as run, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(idle.main([]), 0)
            self.assertTrue((Path(directory) / "state.json").exists())
            run.assert_called_once_with(["/usr/bin/systemctl", "poweroff"], check=True, timeout=10)

    def test_work_appearing_during_final_recheck_prevents_shutdown(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(idle, "STATE_DIR", Path(directory)), \
                patch.object(idle.os, "geteuid", return_value=0), \
                patch.object(idle.os, "umask"), \
                patch.object(idle, "evaluate", side_effect=[({}, {"would_poweroff": True}),
                                                          ({"idle_since": None}, {"would_poweroff": False})]), \
                patch.object(idle.subprocess, "run") as run, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(idle.main([]), 0)
            run.assert_not_called()

    def test_state_write_failure_prevents_shutdown(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(idle, "STATE_DIR", Path(directory)), \
                patch.object(idle.os, "geteuid", return_value=0), \
                patch.object(idle.os, "umask"), \
                patch.object(idle, "evaluate", return_value=({}, {"would_poweroff": True})), \
                patch.object(idle, "save_state", side_effect=OSError("read-only")), \
                patch.object(idle.subprocess, "run") as run, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(idle.main([]), 1)
            run.assert_not_called()

    def test_failed_inspection_disarms_previous_grace(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(idle, "STATE_DIR", Path(directory)), \
                patch.object(idle.os, "geteuid", return_value=0), \
                patch.object(idle.os, "umask"), \
                patch.object(idle, "evaluate", side_effect=OSError("proc unavailable")), \
                patch.object(idle.subprocess, "run") as run, \
                contextlib.redirect_stdout(io.StringIO()):
            path = Path(directory) / "state.json"
            path.write_text('{"idle_since": 0, "checked_at": 1800, "boot_id": "boot"}')
            self.assertEqual(idle.main([]), 1)
            self.assertEqual(json.loads(path.read_text()), {})
            run.assert_not_called()


class ApiTests(unittest.TestCase):
    def probe(self, script, timeout=2):
        """Use real pipes/process cleanup, never invoke real runuser or Jcode."""
        real_popen = subprocess.Popen
        children = []

        def spawn(command, **kwargs):
            self.assertEqual(command[:4], ["/usr/sbin/runuser", "-u", "ec2-user", "--"])
            self.assertEqual(command[-2:], ["api", "--stdio"])
            process = real_popen([sys.executable, "-u", "-c", script], **kwargs)
            children.append(process)
            return process

        try:
            with patch.object(idle.socket, "socket"), patch.object(idle.subprocess, "Popen", side_effect=spawn):
                return idle.api_sessions(timeout=timeout, runtime_env={"JCODE_SOCKET": "/fixture/jcode.sock"})
        finally:
            for child in children:
                self.assertIsNotNone(child.poll(), "probe child must be reaped")

    def test_exact_wire_hello_then_list_without_attaching(self):
        result = self.probe('''
import sys, json
hello = json.loads(sys.stdin.readline())
assert hello['req'] == 'hello' and hello['min_version'] == 1
print(json.dumps({'v': 1, 'reply_to': 1, 'ev': 'hello_ok', 'version': 1}), flush=True)
request = json.loads(sys.stdin.readline())
assert request == {'v': 1, 'id': 2, 'req': 'list_sessions', 'include_archived': True}
print(json.dumps({'v': 1, 'reply_to': 2, 'ev': 'sessions', 'sessions': [{'status': 'idle'}]}), flush=True)
sys.stdin.read()
''')
        self.assertFalse(result["authoritative"])
        self.assertEqual(result["sessions"], [{"status": "idle", "swarm_status": None}])

    def test_timeout_kills_and_reaps_child(self):
        with self.assertRaises(TimeoutError):
            self.probe("import time; time.sleep(60)", timeout=0.1)

    def test_malformed_error_and_eof_fail_safe(self):
        for script in ('print("not-json", flush=True)',
                       'print(\'{"v":1,"ev":"error"}\', flush=True)',
                       'pass'):
            with self.subTest(script=script), self.assertRaises(ValueError):
                self.probe(script)

    def test_missing_runtime_never_spawns(self):
        with patch.object(idle.subprocess, "Popen") as spawn, self.assertRaises(ValueError):
            idle.api_sessions()
        spawn.assert_not_called()

    def test_runtime_path_follows_daemon_environment(self):
        cases = [(b'USER=ec2-user\0', '/tmp/jcode-ec2-user/jcode.sock'),
                 (b'USER=ec2-user\0UID=1000\0TMPDIR=/custom\0', '/custom/jcode-1000/jcode.sock'),
                 (b'XDG_RUNTIME_DIR=/run/user/1000\0', '/run/user/1000/jcode.sock'),
                 (b'JCODE_RUNTIME_DIR=/runtime\0JCODE_SOCKET=/specific.sock\0', '/specific.sock')]
        for raw, expected in cases:
            with patch.object(Path, "read_bytes", return_value=raw):
                env = idle.runtime_environment({10: row()}, [10], 1000)
                self.assertEqual(env['JCODE_SOCKET'], expected)


class BackgroundTests(unittest.TestCase):
    def tasks(self, record, rows=None):
        with tempfile.TemporaryDirectory() as directory:
            status = Path(directory) / "job.status.json"
            status.write_text(json.dumps(record))
            visited = []

            def listing(path):
                visited.append(str(path))
                return iter([status]) if path == Path('/tmp/jcode-bg-tasks') else iter([])

            with patch.object(Path, "iterdir", listing):
                result = idle.background_tasks(rows or {}, [])
            self.assertIn('/tmp/jcode-bg-tasks', visited)
            return result

    def test_actual_temp_directory_status_file_and_missing_pid(self):
        tasks = self.tasks({"status": "running", "pid": None})
        self.assertEqual(tasks, [{"status": "running", "pid": None, "pid_alive": False}])
        observation = empty()
        observation['tasks'] = tasks
        self.assertFalse(idle.policy(observation)['idle'])

    def test_live_pid_in_terminal_file_is_retained(self):
        self.assertTrue(self.tasks({"status": "completed", "pid": 9}, {9: row(0)})[0]['pid_alive'])

    def test_corrupt_or_invalid_pid_records_raise(self):
        for record in (None, [], {"status": "completed", "pid": "9"},
                       {"status": "completed", "pid": True}):
            with self.subTest(record=record), self.assertRaises(ValueError):
                self.tasks(record)


if __name__ == "__main__":
    unittest.main()
