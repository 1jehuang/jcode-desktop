#!/usr/bin/env python3
"""Combined-gate fixtures execute real CLI/fake gh without contacting GitHub."""
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

from test_verify_0_2_1_native_gates import FAKE_GH

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/verify-0.2.1-recovery-gates.py"
SPEC = importlib.util.spec_from_file_location("combined_gate", SCRIPT)
G = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(G)


def endpoint(run_id):
    return f"repos/{G.REPOSITORY}/actions/runs/{run_id}"


def steps(names):
    return [{"number": i, "name": name, "status": "completed", "conclusion": "success"}
            for i, name in enumerate(names, 1)]


def fixtures():
    result = {}
    for contract in G.contracts():
        run_id, sha, branch, workflow, conclusion, names = contract
        result[endpoint(run_id)] = [{
            "id": run_id, "run_attempt": 1, "head_sha": sha, "head_branch": branch,
            "path": f".github/workflows/{workflow}", "repository": {"full_name": G.REPOSITORY},
            "head_repository": {"full_name": G.REPOSITORY},
            "event": "workflow_dispatch" if run_id in {G.FREEBSD_RUN, G.WINDOWS_RUN} else "push",
            "status": "completed", "conclusion": conclusion,
        }]
        rows = []
        for i, (name, conclusions) in enumerate(names.items(), 1):
            job = {"id": run_id + i, "name": name, "run_id": run_id, "run_attempt": 1,
                   "head_sha": sha, "head_branch": branch, "status": "completed",
                   "conclusion": conclusions[0], "steps": []}
            if run_id == G.BASE.MAC_RUN:
                job["steps"] = steps(G.MAC_REQUIRED)
            elif run_id == G.FREEBSD_RUN:
                job["steps"] = steps(G.FREEBSD_REQUIRED)
            elif run_id == G.WINDOWS_RUN:
                job["id"] = G.WINDOWS_JOB_IDS[name]
                job["steps"] = steps(G.WINDOWS_REQUIRED)
            elif name in G.ORIGINAL_WINDOWS_IDS:
                job.update(id=G.ORIGINAL_WINDOWS_IDS[name], conclusion="failure")
                job["steps"] = [{"number": n, "name": value, "status": "completed",
                                 "conclusion": "skipped" if n in {*range(8, 16), 19}
                                 else "failure" if n == 20 else "success"}
                                for n, value in G.ORIGINAL_WINDOWS_STEPS.items()]
                job["steps"].append({"number": 41, "name": "Complete job",
                                     "status": "completed", "conclusion": "success"})
            elif name in G.BASE.MATRIX_JOBS[:2]:
                job["steps"] = steps(G.LINUX_REQUIRED)
            elif name == G.BASE.FREEBSD_JOB:
                job["id"] = G.BASE.FREEBSD_JOB_ID
            rows.append(job)
        result[endpoint(run_id) + "/attempts/1/jobs?per_page=100"] = [[{"total_count": len(rows), "jobs": rows}]]
    return result


def jobs(data, run_id):
    return data[endpoint(run_id) + "/attempts/1/jobs?per_page=100"][0][0]["jobs"]


def pending(data, run_id):
    data[endpoint(run_id)][0].update(status="in_progress", conclusion=None)
    jobs(data, run_id)[0].update(status="in_progress", conclusion=None)


class RecoveryGateTests(unittest.TestCase):
    def execute(self, data, *args):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fixtures.json").write_text(json.dumps(data))
            (root / "gh").write_text(FAKE_GH)
            (root / "gh").chmod(0o755)
            env = dict(os.environ, PATH=f"{root}{os.pathsep}{os.environ['PATH']}",
                       GATE_FIXTURES=str(root), PYTHONDONTWRITEBYTECODE="1")
            result = subprocess.run([sys.executable, str(SCRIPT), *args], env=env,
                                    text=True, capture_output=True, timeout=20)
            state_path = root / "state.json"
            state = json.loads(state_path.read_text()) if state_path.exists() else {}
            return result, state

    def test_unpinned_windows_identity_refuses_before_api(self):
        with mock.patch.object(G, "WINDOWS_RUN", None), mock.patch.object(G.BASE, "gh_json") as api:
            with self.assertRaises(G.GateError):
                G.observe()
            api.assert_not_called()

    def test_complete_success_and_pinned_read_only_requests(self):
        result, state = self.execute(fixtures())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(state), 8)
        self.assertIn('"ready": true', result.stdout)
        self.assertIn("Complete-asset and publication gates remain required", result.stdout)
        self.assertIn(str(G.FREEBSD_RUN), result.stdout)
        self.assertIn(G.FREEBSD_SHA, result.stdout)
        self.assertNotIn("35505409432", result.stdout)

    def test_recovered_runs_require_whole_success(self):
        for run_id in (G.FREEBSD_RUN, G.WINDOWS_RUN):
            for conclusion in ("failure", "cancelled", "timed_out", "skipped", None):
                with self.subTest(run=run_id, conclusion=conclusion):
                    data = fixtures()
                    data[endpoint(run_id)][0]["conclusion"] = conclusion
                    self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_wrong_recovery_identity_rejected(self):
        for run_id in (G.FREEBSD_RUN, G.WINDOWS_RUN):
            for key, value in {"id": 35505409432, "head_sha": G.SOURCE_SHA,
                               "head_branch": "main", "run_attempt": 2, "event": "push",
                               "path": ".github/workflows/evil.yml",
                               "repository": {"full_name": "evil/fork"}}.items():
                with self.subTest(run=run_id, key=key):
                    data = fixtures()
                    data[endpoint(run_id)][0][key] = value
                    self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_recovery_native_source_and_upload_steps_all_mandatory(self):
        for run_id, required in ((G.FREEBSD_RUN, G.FREEBSD_REQUIRED), (G.WINDOWS_RUN, G.WINDOWS_REQUIRED)):
            for name in required:
                with self.subTest(run=run_id, step=name):
                    data = fixtures()
                    step = next(s for s in jobs(data, run_id)[0]["steps"] if s["name"] == name)
                    step["conclusion"] = "skipped"
                    self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_mac_and_both_linux_success_still_mandatory(self):
        for run_id, name in ((G.BASE.MAC_RUN, "build"),
                             *[(G.BASE.CROSS_RUN, n) for n in ("prepare", *G.BASE.MATRIX_JOBS[:2])]):
            with self.subTest(name=name):
                data = fixtures()
                next(j for j in jobs(data, run_id) if j["name"] == name)["conclusion"] = "failure"
                self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_original_windows_failure_must_be_transport_only(self):
        for job_name in G.ORIGINAL_WINDOWS_IDS:
            for number in (4, 16, 17, 18, 38, 39, 40):
                with self.subTest(name=job_name, number=number):
                    data = fixtures()
                    job = next(j for j in jobs(data, G.BASE.CROSS_RUN) if j["name"] == job_name)
                    next(s for s in job["steps"] if s["number"] == number)["conclusion"] = "failure"
                    self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_original_windows_success_also_accepted_without_failure_exception(self):
        data = fixtures()
        job = next(j for j in jobs(data, G.BASE.CROSS_RUN) if j["name"] in G.ORIGINAL_WINDOWS_IDS)
        job["conclusion"] = "success"
        next(s for s in job["steps"] if s["number"] == 20)["conclusion"] = "success"
        self.assertEqual(self.execute(data)[0].returncode, 0)

    def test_original_windows_incomplete_or_renamed_or_duplicate_step_rejected(self):
        for mode in ("missing", "name", "number", "duplicate", "wrong-id", "extra-failure"):
            with self.subTest(mode=mode):
                data = fixtures()
                job = next(j for j in jobs(data, G.BASE.CROSS_RUN) if j["name"] in G.ORIGINAL_WINDOWS_IDS)
                if mode == "missing":
                    job["steps"].pop(17)
                elif mode == "name":
                    job["steps"][17]["name"] = "echo passed"
                elif mode == "number":
                    job["steps"][17]["number"] = 100
                elif mode == "duplicate":
                    job["steps"].append(copy.deepcopy(job["steps"][17]))
                elif mode == "wrong-id":
                    job["id"] = 1
                else:
                    job["steps"].append({"number": 99, "name": "other failure", "status": "completed", "conclusion": "failure"})
                self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_missing_duplicate_unexpected_recovery_jobs_fail(self):
        for mode in ("missing", "duplicate", "unexpected", "wrong-sha"):
            data = fixtures()
            key = endpoint(G.WINDOWS_RUN) + "/attempts/1/jobs?per_page=100"
            rows = jobs(data, G.WINDOWS_RUN)
            if mode == "missing":
                rows.pop()
            elif mode == "duplicate":
                rows.append(copy.deepcopy(rows[0]))
            elif mode == "unexpected":
                rows[0]["name"] = "$(touch /NOT_EXECUTED)"
            else:
                rows[0]["head_sha"] = "bad"
            data[key][0][0]["total_count"] = len(rows)
            self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_pending_does_not_hide_other_failure(self):
        data = fixtures()
        pending(data, G.FREEBSD_RUN)
        result, state = self.execute(data)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn(endpoint(G.WINDOWS_RUN), state)
        jobs(data, G.WINDOWS_RUN)[0]["conclusion"] = "failure"
        self.assertEqual(self.execute(data, "--timeout-minutes", "1")[0].returncode, 1)

    def test_polling_observes_fresh_success(self):
        ready, waiting = fixtures(), fixtures()
        pending(waiting, G.FREEBSD_RUN)
        data = {key: waiting[key] * (1 if "/jobs?" in key else 2) + ready[key] * 2 for key in ready}
        result, state = self.execute(data, "--timeout-minutes", "0.2", "--poll-seconds", "0.01")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(state[endpoint(G.FREEBSD_RUN)], 4)

    def test_metadata_rerun_race_refuses(self):
        data = fixtures()
        key = endpoint(G.FREEBSD_RUN)
        changed = copy.deepcopy(data[key][0])
        changed["run_attempt"] = 2
        data[key].append(changed)
        self.assertEqual(self.execute(data)[0].returncode, 1)

    def test_bounds_and_api_failure_refuse(self):
        for number in ("181", "nan", "-1"):
            result, state = self.execute(fixtures(), "--timeout-minutes", number)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(state, {})
        data = fixtures()
        data[endpoint(G.FREEBSD_RUN)] = [{"exit": 7}]
        self.assertEqual(self.execute(data)[0].returncode, 1)


if __name__ == "__main__":
    unittest.main()
