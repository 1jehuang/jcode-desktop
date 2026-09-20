#!/usr/bin/env python3
"""Model-only sync regression tests. All credentials below are synthetic."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import Mock, patch

import model_sync as sync

SECRET = "fake-provider-secret-for-tests"
UNRELATED = "must-never-leave-local-home"
CONFIG = dict(account_id="123456789012", instance_id="i-0123456789abcdef0", profile="personal", region="us-east-1")


class ModelSyncTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name) / "local"
        self.remote = Path(self.tmp.name) / "remote"
        self.home.mkdir(mode=0o700)
        self.remote.mkdir(mode=0o700)
        self.put(".ssh/jcode_cloud_alpha_known_hosts", "jcode-cloud-alpha ssh-ed25519 fake-public-key\n")
        self.put(".ssh/config", "Host jcode-cloud-alpha\n HostName i-0123456789abcdef0\n")
        self.put(".jcode/openai-auth.json", json.dumps({
            "openai_accounts": [{"label": "openai-otter", "access_token": SECRET,
                                 "refresh_token": "fake-refresh", "expires_at": 9999999999999,
                                 "account_id": "fake-account", "email": UNRELATED,
                                 "account_login_token": UNRELATED}],
            "active_openai_account": "openai-otter", "account_token": UNRELATED}))
        self.put(".config/jcode/openai.env", "OPENAI_API_KEY=" + SECRET + "\nAWS_SECRET_ACCESS_KEY=" + UNRELATED + "\n")
        self.put(".config/jcode/nari.env", "NARI_API_KEY=" + UNRELATED)
        self.put(".jcode/google_oauth.json", json.dumps({"access_token": UNRELATED}))
        self.put(".jcode/config.toml", '[provider]\ndefault_model="openai:test-model"\nopenai_reasoning_effort="high"\n[agents]\nswarm_model="inherit"\nswarm_spawn_mode="headless"\n[hooks]\nspawn="' + UNRELATED + '"\n')
        self.runner = Mock(return_value=subprocess.CompletedProcess([], 0, b"model-sync-ok\n"))
        self.verify = Mock()

    def put(self, name, data, remote=False):
        path = (self.remote if remote else self.home) / name
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.write_text(data)
        path.chmod(0o600)
        return path

    def snapshot(self, environ=None):
        return sync.collect(self.home, {} if environ is None else environ)

    def call_sync(self, **kwargs):
        options = dict(home=self.home, environ={}, runner=self.runner, verify=self.verify, now=100)
        options.update(kwargs)
        return sync.sync_personal_alpha(CONFIG, **options)

    def test_strict_projection_never_copies_unrelated_credentials(self):
        payload = self.snapshot({"AWS_SECRET_ACCESS_KEY": UNRELATED, "GITHUB_TOKEN": UNRELATED,
                                 "JCODE_ACCOUNT_TOKEN": UNRELATED})
        self.assertNotIn(UNRELATED.encode(), sync.canonical(payload))
        self.assertEqual(set(payload["oauth"]), {"openai-auth.json"})
        self.assertEqual(payload["env"], {"openai.env": {"OPENAI_API_KEY": SECRET}})
        self.assertNotIn("hooks", payload["settings"])
        self.assertNotIn("swarm_spawn_mode", payload["settings"]["agents"])

    def test_environment_api_key_wins_without_copying_environment(self):
        payload = self.snapshot({"OPENAI_API_KEY": "env-secret", "PATH": UNRELATED})
        self.assertEqual(payload["env"]["openai.env"]["OPENAI_API_KEY"], "env-secret")
        self.assertNotIn(UNRELATED.encode(), sync.canonical(payload))

    def test_jcode_home_and_xdg_source_roots(self):
        isolated = self.home / "isolated"
        self.put("isolated/config/jcode/openai.env", "OPENAI_API_KEY=isolated-secret\n")
        payload = self.snapshot({"JCODE_HOME": str(isolated)})
        self.assertEqual(payload["env"]["openai.env"]["OPENAI_API_KEY"], "isolated-secret")
        self.assertFalse(payload["oauth"])
        self.put("xdg/jcode/openai.env", "OPENAI_API_KEY=xdg-secret\n")
        self.assertEqual(self.snapshot({"XDG_CONFIG_HOME": str(self.home / "xdg")})["env"]["openai.env"]["OPENAI_API_KEY"], "xdg-secret")

    def test_transport_uses_pinned_stdin_not_argv_or_logs(self):
        self.assertTrue(self.call_sync())
        args, options = self.runner.call_args
        argv = args[0]
        self.assertNotIn(SECRET, " ".join(argv))
        self.assertNotIn(UNRELATED, " ".join(argv))
        self.assertIn(SECRET.encode(), options["input"])
        self.assertIn("StrictHostKeyChecking=yes", argv)
        self.assertIn("HostName=" + CONFIG["instance_id"], argv)
        self.assertIn("HostKeyAlias=jcode-cloud-alpha", argv)
        self.assertEqual(options["stderr"], subprocess.DEVNULL)
        self.assertEqual(options["timeout"], 45)
        self.verify.assert_called_once_with(CONFIG)
        cache = (self.home / ".jcode/cloud-model-sync-local.json").read_bytes()
        self.assertNotIn(SECRET.encode(), cache)
        self.assertEqual((self.home / ".jcode/cloud-model-sync-local.json").stat().st_mode & 0o777, 0o600)

    def test_warm_hash_cache_avoids_aws_and_ssh(self):
        self.call_sync()
        self.assertFalse(self.call_sync(now=101))
        self.assertEqual(self.runner.call_count, 1)
        self.assertEqual(self.verify.call_count, 1)
        self.assertTrue(self.call_sync(now=131))
        self.assertEqual(self.runner.call_count, 2)

    def test_changed_local_key_invalidates_warm_cache(self):
        self.call_sync()
        self.put(".config/jcode/openai.env", "OPENAI_API_KEY=changed\n")
        self.assertTrue(self.call_sync(now=101))
        self.assertEqual(self.runner.call_count, 2)

    def test_changed_pin_and_clock_rollback_invalidate_cache(self):
        self.call_sync()
        self.put(".ssh/jcode_cloud_alpha_known_hosts", "changed-public-pin")
        self.assertTrue(self.call_sync(now=101))
        self.assertTrue(self.call_sync(now=99))

    def test_failure_does_not_cache_or_leak_remote_output(self):
        self.runner.return_value = subprocess.CompletedProcess([], 1, SECRET.encode(), SECRET.encode())
        with self.assertRaises(sync.SyncError) as caught:
            self.call_sync()
        self.assertNotIn(SECRET, str(caught.exception))
        self.assertFalse((self.home / ".jcode/cloud-model-sync-local.json").exists())

    def test_parser_and_timeout_errors_redacted(self):
        self.runner.side_effect = subprocess.TimeoutExpired([SECRET], 1, stderr=SECRET.encode())
        with self.assertRaises(sync.SyncError) as caught:
            self.call_sync()
        self.assertNotIn(SECRET, str(caught.exception))
        self.put(".jcode/openai-auth.json", SECRET)
        with self.assertRaises(sync.SyncError) as caught:
            self.call_sync()
        self.assertNotIn(SECRET, str(caught.exception))

    def test_identity_failure_prevents_transfer(self):
        self.verify.side_effect = RuntimeError(SECRET)
        with self.assertRaises(sync.SyncError):
            self.call_sync()
        self.runner.assert_not_called()

    def test_source_symlink_fifo_and_oversize_refused(self):
        source = self.home / ".jcode/openai-auth.json"
        source.unlink()
        source.symlink_to(self.home / ".jcode/google_oauth.json")
        with self.assertRaises(sync.SyncError):
            self.call_sync()
        source.unlink()
        os.mkfifo(source)
        with self.assertRaises(sync.SyncError):
            self.call_sync()
        source.unlink()
        source.write_bytes(b" " * (sync.LIMIT + 1))
        with self.assertRaises(sync.SyncError):
            self.call_sync()
        self.runner.assert_not_called()

    def test_ancestor_symlink_refused(self):
        alias = self.home / "alias"
        alias.symlink_to(self.home / ".jcode", target_is_directory=True)
        with self.assertRaises(sync.SyncError):
            self.call_sync(environ={"JCODE_HOME": str(alias)})

    def test_apply_preserves_remote_unrelated_settings_and_env(self):
        self.put(".jcode/config.toml", '[hooks]\nspawn="remote-only"\n[provider]\ndefault_model="old"\n[agents]\nswarm_spawn_mode="headless"\n', remote=True)
        self.put(".config/jcode/openai.env", "OTHER=remote-only\nOPENAI_API_KEY=old\n", remote=True)
        self.assertEqual(sync.apply_snapshot(self.remote, self.snapshot()), 3)
        config = tomllib.loads((self.remote / ".jcode/config.toml").read_text())
        self.assertEqual(config["hooks"]["spawn"], "remote-only")
        self.assertEqual(config["agents"]["swarm_spawn_mode"], "headless")
        self.assertEqual(config["provider"]["default_model"], "openai:test-model")
        self.assertIn("OTHER=remote-only", (self.remote / ".config/jcode/openai.env").read_text())
        for path in (self.remote / ".jcode").glob("*"):
            if path.is_file():
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_unchanged_input_preserves_remote_oauth_refresh(self):
        payload = self.snapshot()
        sync.apply_snapshot(self.remote, payload)
        path = self.remote / ".jcode/openai-auth.json"
        refreshed = json.loads(path.read_text())
        refreshed["openai_accounts"][0]["refresh_token"] = "remote-rotated"
        path.write_text(json.dumps(refreshed))
        before = path.stat().st_mtime_ns
        self.assertEqual(sync.apply_snapshot(self.remote, payload), 0)
        self.assertEqual(json.loads(path.read_text()), refreshed)
        self.assertEqual(path.stat().st_mtime_ns, before)
        payload["settings"]["provider"]["default_model"] = "other-model"
        sync.apply_snapshot(self.remote, payload)
        self.assertEqual(json.loads(path.read_text()), refreshed)

    def test_account_selection_change_preserves_remote_rotations(self):
        payload = self.snapshot()
        second = dict(payload["oauth"]["openai-auth.json"]["openai_accounts"][0], label="openai-lynx")
        payload["oauth"]["openai-auth.json"]["openai_accounts"].append(second)
        sync.apply_snapshot(self.remote, payload)
        path = self.remote / ".jcode/openai-auth.json"
        refreshed = json.loads(path.read_text())
        refreshed["openai_accounts"][0]["refresh_token"] = "remote-rotated"
        path.write_text(json.dumps(refreshed))
        payload["oauth"]["openai-auth.json"]["active_openai_account"] = "openai-lynx"
        with self.assertRaises(sync.SyncError):
            sync.apply_snapshot(self.remote, payload)
        result = json.loads(path.read_text())
        self.assertEqual(result["openai_accounts"][0]["refresh_token"], "remote-rotated")
        self.assertEqual(result["active_openai_account"], "openai-otter")

    def test_destination_symlink_preflight_prevents_all_writes(self):
        target = self.put("outside", "untouched", remote=True)
        dest = self.remote / ".jcode/config.toml"
        dest.parent.mkdir(mode=0o700)
        dest.symlink_to(target)
        with self.assertRaises(OSError):
            sync.apply_snapshot(self.remote, self.snapshot())
        self.assertEqual(target.read_text(), "untouched")
        self.assertFalse((self.remote / ".jcode/openai-auth.json").exists())

    def test_malformed_remote_config_prevents_partial_credentials(self):
        self.put(".jcode/config.toml", "not valid toml", remote=True)
        with self.assertRaises(Exception):
            sync.apply_snapshot(self.remote, self.snapshot())
        self.assertFalse((self.remote / ".jcode/openai-auth.json").exists())

    def test_payload_cannot_escape_allowlists(self):
        for section, name, value in [("oauth", "../escape", {}), ("env", "aws.env", {"AWS_SECRET_ACCESS_KEY": SECRET}),
                                      ("settings", "hooks", {"spawn": "bad"})]:
            payload = self.snapshot()
            payload[section][name] = value
            with self.assertRaises(sync.SyncError):
                sync.apply_snapshot(self.remote, payload)
        payload = self.snapshot()
        payload["env"]["openai.env"]["AWS_SECRET_ACCESS_KEY"] = SECRET
        with self.assertRaises(sync.SyncError):
            sync.validate(payload)

    def test_api_key_newline_rejected(self):
        with self.assertRaises(sync.SyncError):
            self.snapshot({"OPENAI_API_KEY": "key\nAWS_SECRET_ACCESS_KEY=oops"})

    def test_named_profiles_fail_closed(self):
        self.put(".jcode/config.toml", '[providers.gateway]\napi_key="' + SECRET + '"\n')
        with self.assertRaises(sync.SyncError) as caught:
            self.call_sync()
        self.assertNotIn(SECRET, str(caught.exception))
        self.runner.assert_not_called()

    def test_receiver_subprocess_real_stdin_and_private_files(self):
        result = subprocess.run([sys.executable, "-c", sync.receiver_source(), "--receive"],
                                input=sync.canonical(self.snapshot()), capture_output=True,
                                env={"HOME": str(self.remote), "PATH": os.environ["PATH"], "JCODE_SOCKET": str(self.remote / "missing.sock")}, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(result.stdout, b"model-sync-ok\n")
        self.assertNotIn(SECRET.encode(), result.stdout + result.stderr)
        self.assertTrue((self.remote / ".jcode/openai-auth.json").exists())

    def test_receiver_malformed_stdin_never_echoes_secrets(self):
        for data in [SECRET.encode(), b" " * (sync.LIMIT + 1)]:
            result = subprocess.run([sys.executable, "-c", sync.receiver_source(), "--receive"],
                                    input=data, capture_output=True, env={"HOME": str(self.remote), "JCODE_SOCKET": str(self.remote / "missing.sock")}, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertNotIn(SECRET.encode(), result.stdout + result.stderr)
            self.assertFalse((self.remote / ".jcode/openai-auth.json").exists())

    def test_existing_differing_oauth_refuses_before_any_write(self):
        path = self.put(".jcode/openai-auth.json", '{"openai_accounts": []}', remote=True)
        before = path.read_bytes()
        with self.assertRaises(sync.SyncError):
            sync.apply_snapshot(self.remote, self.snapshot())
        self.assertEqual(path.read_bytes(), before)
        self.assertFalse((self.remote / ".config/jcode/openai.env").exists())

    def test_no_replace_publication_protects_concurrent_native_writer(self):
        fd = sync.directory(self.remote)
        self.addCleanup(os.close, fd)
        real_link = os.link
        def concurrent(src, dst, **kwargs):
            (self.remote / dst).write_bytes(b"native-refresh-wins")
            return real_link(src, dst, **kwargs)
        with patch.object(sync.os, "link", side_effect=concurrent):
            with self.assertRaises(FileExistsError):
                sync.publish(fd, "oauth.json", b"local-copy", no_replace=True)
        self.assertEqual((self.remote / "oauth.json").read_bytes(), b"native-refresh-wins")
        self.assertFalse(list(self.remote.glob(".model-sync-*")))

    def test_effective_model_environment_overrides(self):
        settings = self.snapshot({"JCODE_MODEL": "env-model", "JCODE_PROVIDER": "openai",
                                  "JCODE_OPENAI_REASONING_EFFORT": "max", "JCODE_SWARM_MODEL": "worker-model",
                                  "JCODE_GEMINI_FORCE_OAUTH": "true", "GOOGLE_CLOUD_PROJECT": "fake-project"})["settings"]
        self.assertEqual(settings["provider"]["default_model"], "env-model")
        self.assertEqual(settings["provider"]["openai_reasoning_effort"], "max")
        self.assertEqual(settings["agents"]["swarm_model"], "worker-model")
        self.assertIs(settings["provider"]["gemini_force_oauth"], True)
        self.assertEqual(settings["provider"]["gemini_project"], "fake-project")

    def test_whitespace_environment_key_falls_back_to_file(self):
        self.assertEqual(self.snapshot({"OPENAI_API_KEY": "  "})["env"]["openai.env"]["OPENAI_API_KEY"], SECRET)

    def test_invalid_native_value_types_refused(self):
        for bad in ({"default_model": 5}, {"max_retries": "bad"}, {"gemini_force_oauth": "false"}):
            payload = self.snapshot()
            payload["settings"]["provider"] = bad
            with self.assertRaises(sync.SyncError):
                sync.validate(payload)
        payload = self.snapshot()
        payload["oauth"]["gemini_oauth.json"] = {"access_token": "", "refresh_token": 42, "expires_at": "bad"}
        with self.assertRaises(sync.SyncError):
            sync.validate(payload)

    def test_runtime_refresh_is_retried_on_failure_then_deduplicated(self):
        refresher = Mock(side_effect=RuntimeError("simulated runtime failure"))
        with self.assertRaises(RuntimeError):
            sync.apply_snapshot(self.remote, self.snapshot(), refresh=refresher)
        self.assertFalse((self.remote / ".jcode/cloud-model-sync-state.json").exists())
        refresher.side_effect = None
        sync.apply_snapshot(self.remote, self.snapshot(), refresh=refresher)
        sync.apply_snapshot(self.remote, self.snapshot(), refresh=refresher)
        self.assertEqual(refresher.call_count, 2)

    def test_native_refresh_waits_for_completion_not_done(self):
        import socket
        import threading
        path = self.remote / "daemon.sock"
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.addCleanup(server.close)
        server.bind(str(path))
        server.listen(1)
        received = []
        errors = []
        def run_server():
            try:
                with server.accept()[0] as client:
                    with client.makefile("rb") as stream:
                        for _ in range(2):
                            request = json.loads(stream.readline())
                            received.append(request)
                            client.sendall(sync.canonical({"type": "done", "id": request["id"]}) + b"\n")
                            client.settimeout(0.03)
                            with self.assertRaises(TimeoutError):
                                client.recv(1, socket.MSG_PEEK)
                            client.settimeout(5)
                            event = {"type": "notification", "notification_type": {"kind": "message", "scope": "catalog_activity"},
                                     "message": "**Model access refreshed**\nTest catalog unchanged"}
                            client.sendall(sync.canonical(event) + b"\n")
            except BaseException as error:
                errors.append(error)
        worker = threading.Thread(target=run_server)
        worker.start()
        sync.refresh_daemon(self.snapshot(), [path])
        worker.join(timeout=5)
        self.assertFalse(worker.is_alive())
        self.assertFalse(errors, errors)
        self.assertEqual([r["provider"] for r in received], ["openai", "openai-api"])
        self.assertNotIn(SECRET.encode(), sync.canonical(received))

    def test_toml_roundtrip_nested_tables_lists_and_quotes(self):
        value = {"hooks": {"spawn": "echo 'hi'\nnext"}, "tables": [{"a": [True, 1, "x"]}], "provider": {"default_model": "test"}}
        self.assertEqual(tomllib.loads(sync.toml_document(value).decode()), value)


if __name__ == "__main__":
    unittest.main()
