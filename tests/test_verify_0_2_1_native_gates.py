#!/usr/bin/env python3
"""Execute the real native gate CLI against a temporary fake gh, without CI/API calls."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/verify-0.2.1-native-gates.py"
SPEC = importlib.util.spec_from_file_location("native_gate", SCRIPT)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def run_fixture(run_id):
    return {
        "id": run_id, "run_attempt": 1, "head_sha": GATE.SHA,
        "head_branch": GATE.TAG, "path": GATE.WORKFLOWS[run_id],
        "repository": {"full_name": GATE.REPOSITORY},
        "head_repository": {"full_name": GATE.REPOSITORY},
        "event": "push", "status": "completed",
        "conclusion": "success" if run_id == GATE.MAC_RUN else "failure",
    }


def jobs_fixture(run_id):
    pairs = [("build", "success")] if run_id == GATE.MAC_RUN else [
        ("prepare", "success"), *[(name, "success") for name in GATE.MATRIX_JOBS],
        (GATE.FREEBSD_JOB, "failure"), ("publish", "skipped"),
    ]
    jobs = [{"id": GATE.FREEBSD_JOB_ID if name == GATE.FREEBSD_JOB else run_id + i,
             "name": name, "run_id": run_id, "run_attempt": 1,
             "head_sha": GATE.SHA, "head_branch": GATE.TAG,
             "status": "completed", "conclusion": conclusion}
            for i, (name, conclusion) in enumerate(pairs)]
    return [{"total_count": len(jobs), "jobs": jobs}]


def endpoint(run_id):
    return f"repos/{GATE.REPOSITORY}/actions/runs/{run_id}"


def fixture():
    result = {}
    for run_id in (GATE.MAC_RUN, GATE.CROSS_RUN):
        result[endpoint(run_id)] = [run_fixture(run_id)]
        result[endpoint(run_id) + "/attempts/1/jobs?per_page=100"] = [jobs_fixture(run_id)]
    return result


def jobs(data, run_id=GATE.CROSS_RUN):
    return data[endpoint(run_id) + "/attempts/1/jobs?per_page=100"][0][0]["jobs"]


def pending(data, run_id=GATE.MAC_RUN):
    data[endpoint(run_id)][0].update(status="in_progress", conclusion=None)
    jobs(data, run_id)[0].update(status="in_progress", conclusion=None)


FAKE_GH = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ["GATE_FIXTURES"])
args = sys.argv[1:]
assert args[:3] == ["api", "--hostname", "github.com"], args
route = args[3]
assert args[4:] == (["--paginate", "--slurp"] if "/jobs?" in route else []), args
state_path = root / "state.json"
state = json.loads(state_path.read_text()) if state_path.exists() else {}
index = state.get(route, 0)
state[route] = index + 1
state_path.write_text(json.dumps(state))
fixtures = json.loads((root / "fixtures.json").read_text())
values = fixtures[route]
value = values[min(index, len(values) - 1)]
if isinstance(value, dict) and "exit" in value:
    sys.exit(value["exit"])
if isinstance(value, dict) and "raw" in value:
    print(value["raw"])
else:
    print(json.dumps(value))
'''


class NativeGateTests(unittest.TestCase):
    def execute(self, data, *args):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fixtures.json").write_text(json.dumps(data))
            gh = root / "gh"
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            env = dict(os.environ, PATH=f"{root}{os.pathsep}{os.environ['PATH']}",
                       GATE_FIXTURES=str(root), PYTHONDONTWRITEBYTECODE="1")
            result = subprocess.run([sys.executable, str(SCRIPT), *args], env=env,
                                    capture_output=True, text=True, timeout=15)
            state = json.loads((root / "state.json").read_text()) if (root / "state.json").exists() else {}
            return result, state

    def test_exact_success_and_read_only_requests(self):
        result, state = self.execute(fixture())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('"ready": true', result.stdout)
        self.assertEqual(len(state), 4)
        self.assertEqual(state[endpoint(GATE.MAC_RUN)], 2)
        self.assertEqual(state[endpoint(GATE.CROSS_RUN)], 2)
        self.assertIn("Recovery and complete-asset gates remain required", result.stdout)

    def test_wrong_run_identity_fields_fail(self):
        for run_id in (GATE.MAC_RUN, GATE.CROSS_RUN):
            for field, value in {
                "id": 7, "run_attempt": 2, "head_sha": "bad",
                "head_branch": "desktop-v0.2.0", "path": ".github/workflows/evil.yml",
                "repository": {"full_name": "attacker/fork"},
                "head_repository": {"full_name": "attacker/fork"},
                "event": "pull_request", "status": "unknown",
            }.items():
                with self.subTest(run=run_id, field=field):
                    data = fixture()
                    data[endpoint(run_id)][0][field] = value
                    result, _ = self.execute(data)
                    self.assertEqual(result.returncode, 1, result.stdout)

    def test_all_unexpected_native_conclusions_fail(self):
        for name in ("prepare", *GATE.MATRIX_JOBS):
            for conclusion in ("failure", "cancelled", "skipped", "timed_out", None):
                with self.subTest(name=name, conclusion=conclusion):
                    data = fixture()
                    next(j for j in jobs(data) if j["name"] == name)["conclusion"] = conclusion
                    result, _ = self.execute(data)
                    self.assertEqual(result.returncode, 1, result.stdout)

    def test_only_exact_original_freebsd_failure_and_skipped_publish_allowed(self):
        for name, field, value in (
            (GATE.FREEBSD_JOB, "id", 7),
            (GATE.FREEBSD_JOB, "conclusion", "cancelled"),
            (GATE.FREEBSD_JOB, "conclusion", "success"),
            ("publish", "conclusion", "success"),
            ("publish", "conclusion", "failure"),
        ):
            with self.subTest(name=name, field=field, value=value):
                data = fixture()
                next(j for j in jobs(data) if j["name"] == name)[field] = value
                self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_missing_unexpected_duplicate_and_malicious_jobs_fail(self):
        for kind in ("missing", "unexpected", "duplicate", "duplicate-id", "malicious", "wrong-job-sha", "wrong-job-tag", "wrong-job-attempt"):
            with self.subTest(kind=kind):
                data = fixture()
                rows = jobs(data)
                if kind == "missing":
                    rows.pop(1)
                elif kind == "unexpected":
                    rows[1]["name"] = "extra job"
                elif kind == "duplicate":
                    rows.append(copy.deepcopy(rows[1]))
                elif kind == "duplicate-id":
                    rows[1]["id"] = rows[0]["id"]
                elif kind == "malicious":
                    rows[1]["name"] = "$(touch /DO_NOT_EXECUTE); build"
                else:
                    key = {"wrong-job-sha": "head_sha", "wrong-job-tag": "head_branch", "wrong-job-attempt": "run_attempt"}[kind]
                    rows[1][key] = "wrong"
                data[endpoint(GATE.CROSS_RUN) + "/attempts/1/jobs?per_page=100"][0][0]["total_count"] = len(rows)
                result, _ = self.execute(data)
                self.assertEqual(result.returncode, 1, result.stdout)

    def test_pagination_requires_complete_unique_set(self):
        data = fixture()
        key = endpoint(GATE.CROSS_RUN) + "/attempts/1/jobs?per_page=100"
        rows = copy.deepcopy(jobs(data))
        data[key] = [[{"total_count": len(rows), "jobs": rows[:3]},
                      {"total_count": len(rows), "jobs": rows[3:]}]]
        self.assertEqual(self.execute(data)[0].returncode, 0)
        data[key][0].pop()
        self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_pending_one_shot_is_nonzero_and_still_checks_other_run(self):
        data = fixture()
        pending(data)
        result, state = self.execute(data)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn(endpoint(GATE.CROSS_RUN), state)
        jobs(data)[1]["conclusion"] = "failure"
        result, _ = self.execute(data, "--timeout-minutes", "1")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("unexpected job conclusion", result.stderr)

    def test_missing_jobs_are_pending_only_while_run_in_progress(self):
        data = fixture()
        data[endpoint(GATE.CROSS_RUN)][0].update(status="in_progress", conclusion=None)
        jobs(data).pop()
        data[endpoint(GATE.CROSS_RUN) + "/attempts/1/jobs?per_page=100"][0][0]["total_count"] -= 1
        self.assertEqual(self.execute(data)[0].returncode, 2)
        data[endpoint(GATE.CROSS_RUN)][0].update(status="completed", conclusion="failure")
        self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_poll_fetches_fresh_json_until_success(self):
        ready = fixture()
        waiting = fixture()
        pending(waiting)
        data = {}
        for key in ready:
            data[key] = waiting[key] * (2 if "/jobs?" not in key else 1) + ready[key] * 2
        result, state = self.execute(data, "--timeout-minutes", "0.2", "--poll-seconds", "0.01")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(state[endpoint(GATE.MAC_RUN)], 4)
        self.assertEqual(state[endpoint(GATE.MAC_RUN) + "/attempts/1/jobs?per_page=100"], 2)

    def test_rerun_race_and_bad_json_and_api_errors_fail(self):
        for kind in ("race", "json", "api"):
            with self.subTest(kind=kind):
                data = fixture()
                key = endpoint(GATE.MAC_RUN)
                if kind == "race":
                    changed = copy.deepcopy(data[key][0])
                    changed["run_attempt"] = 2
                    data[key].append(changed)
                elif kind == "json":
                    data[key] = [{"raw": "not JSON"}]
                else:
                    data[key] = [{"exit": 7}]
                self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_completion_transition_waits_for_fresh_jobs_snapshot(self):
        data = fixture()
        waiting = fixture()
        pending(waiting)
        key = endpoint(GATE.MAC_RUN)
        data[key] = waiting[key] + data[key] * 3
        jobs_key = key + "/attempts/1/jobs?per_page=100"
        data[jobs_key] = waiting[jobs_key] + data[jobs_key]
        result, state = self.execute(data, "--timeout-minutes", "0.2", "--poll-seconds", "0.01")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(state[key], 4)

    def test_boolean_attempt_and_job_run_id_coercion_rejected(self):
        data = fixture()
        data[endpoint(GATE.MAC_RUN)][0]["run_attempt"] = True
        self.assertEqual(self.execute(data)[0].returncode, 1)
        data = fixture()
        jobs(data)[0]["run_attempt"] = True
        self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_macos_final_workflow_must_succeed(self):
        for conclusion in ("failure", "cancelled", "skipped", "timed_out"):
            data = fixture()
            data[endpoint(GATE.MAC_RUN)][0]["conclusion"] = conclusion
            self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_argument_bounds_and_deadline(self):
        for value in ("181", "-1", "nan", "inf"):
            result, state = self.execute(fixture(), "--timeout-minutes", value)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(state, {})
        data = fixture()
        pending(data)
        result, _ = self.execute(data, "--timeout-minutes", "0.005", "--poll-seconds", "0.01")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Native gate refused", result.stderr)
        with mock.patch.object(GATE.time, "monotonic", return_value=10), \
             mock.patch.object(GATE.subprocess, "run") as run:
            with self.assertRaises(GATE.GateError):
                GATE.gh_json("unused", deadline=9)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
