#!/usr/bin/env python3
"""Private model-only synchronization for the explicitly authorized personal alpha.

This is deliberately not a general dotfile copier. No external-tool discovery,
AWS/GitHub/account/Gmail tokens, SSH files, hooks, or integration secrets cross
this boundary. Secrets travel exclusively over pinned SSH stdin. Diagnostics
are static. Import's snapshot ledger keeps remote OAuth refreshes when the
corresponding local input has not changed. No daemon or VM restart is needed.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess
import sys
import time
import uuid

HOST = "jcode-cloud-alpha"
LIMIT = 1024 * 1024
CACHE_TTL = 30
# Explicit built-in model API credentials. Broad HF/GitHub/AWS tokens are excluded.
API_KEYS = {
    "openai.env": ("OPENAI_API_KEY",),
    "anthropic.env": ("ANTHROPIC_API_KEY",),
    "openrouter.env": ("OPENROUTER_API_KEY",),
    "gemini.env": ("GEMINI_API_KEY", "GOOGLE_API_KEY"),
    "groq.env": ("GROQ_API_KEY",),
    "deepseek.env": ("DEEPSEEK_API_KEY",),
    "mistral.env": ("MISTRAL_API_KEY",),
    "xai.env": ("XAI_API_KEY",),
    "cerebras.env": ("CEREBRAS_API_KEY",),
    "togetherai.env": ("TOGETHER_API_KEY",),
    "fireworks.env": ("FIREWORKS_API_KEY",),
    "deepinfra.env": ("DEEPINFRA_API_KEY",),
    "kimi.env": ("KIMI_API_KEY",),
    "moonshotai.env": ("MOONSHOT_API_KEY",),
    "zai.env": ("ZHIPU_API_KEY", "ZAI_API_KEY"),
    "minimax.env": ("MINIMAX_API_KEY",),
    "opencode.env": ("OPENCODE_API_KEY",),
    "opencode-go.env": ("OPENCODE_GO_API_KEY",),
    "302ai.env": ("302AI_API_KEY",),
    "baseten.env": ("BASETEN_API_KEY",),
    "conifer.env": ("CONIFER_API_KEY",),
    "cortecs.env": ("CORTECS_API_KEY",),
    "orcarouter.env": ("ORCAROUTER_API_KEY",),
    "comtegra.env": ("COMTEGRA_API_KEY",),
    "fpt.env": ("FPT_API_KEY",),
    "firmware.env": ("FIRMWARE_API_KEY",),
    "nebius.env": ("NEBIUS_API_KEY",),
    "scaleway.env": ("SCALEWAY_API_KEY",),
    "stackit.env": ("STACKIT_API_KEY",),
    "perplexity.env": ("PERPLEXITY_API_KEY",),
    "novita.env": ("NOVITA_API_KEY",),
    "lmstudio.env": ("LMSTUDIO_API_KEY",),
    "ollama.env": ("OLLAMA_API_KEY",),
    "chutes.env": ("CHUTES_API_KEY",),
    "belvedir.env": ("BELVEDIR_API_KEY",),
    "alibaba-coding-plan.env": ("BAILIAN_CODING_PLAN_API_KEY",),
    "nvidia-nim.env": ("NVIDIA_API_KEY",),
    "xiaomi-mimo.env": ("XIAOMI_MIMO_API_KEY",),
    "meta-muse.env": ("META_MUSE_API_KEY",),
    "celeris.env": ("CELERIS_API_KEY",),
}
OAUTH = {
    "openai-auth.json": ("openai_accounts", "active_openai_account",
                         ("label", "access_token", "refresh_token", "id_token", "account_id", "expires_at")),
    "auth.json": ("anthropic_accounts", "active_anthropic_account",
                  ("label", "access", "refresh", "expires", "scopes", "subscription_type")),
}
PROVIDER_FIELDS = frozenset("default_model default_provider openai_reasoning_effort anthropic_reasoning_effort anthropic_cache_ttl_1h openai_transport openai_service_tier openai_native_compaction_mode openai_native_compaction_threshold_tokens preserve_reasoning_context cross_provider_failover same_provider_account_failover copilot_premium gemini_force_oauth gemini_project model_picker_providers stream_idle_timeout_secs max_retries retry_backoff_cap_secs".split())
BOOL_FIELDS = frozenset("anthropic_cache_ttl_1h preserve_reasoning_context same_provider_account_failover gemini_force_oauth".split())
INT_FIELDS = frozenset("openai_native_compaction_threshold_tokens stream_idle_timeout_secs max_retries retry_backoff_cap_secs".split())
AGENT_FIELDS = frozenset("swarm_model swarm_effort swarm_root_effort swarm_deep_root_effort".split())


class SyncError(RuntimeError):
    """Only fixed messages may be exposed to the desktop helper's stderr."""


class OAuthConflict(SyncError):
    """An existing cloud login must not be overwritten by this importer."""


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def directory(path, create=False):
    """Walk directory descriptors so even ancestor symlinks are rejected."""
    path = Path(path).absolute()
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:]:
            if part in (".", ".."):
                raise SyncError("Unsafe model sync directory.")
            if create:
                try:
                    os.mkdir(part, 0o700, dir_fd=fd)
                except FileExistsError:
                    pass
            nxt = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = nxt
        info = os.fstat(fd)
        if info.st_uid != os.getuid() or info.st_mode & 0o022:
            raise SyncError("Unsafe model sync directory ownership or permissions.")
        return fd
    except BaseException:
        os.close(fd)
        raise


def read_at(fd, name, missing=None):
    try:
        source = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
    except FileNotFoundError:
        return missing
    with os.fdopen(source, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
            raise SyncError("Unsafe model sync file.")
        data = stream.read(LIMIT + 1)
        if len(data) > LIMIT:
            raise SyncError("Model sync file exceeds size limit.")
        return data


def read_path(path, missing=None):
    try:
        fd = directory(Path(path).parent)
    except FileNotFoundError:
        return missing
    try:
        return read_at(fd, Path(path).name, missing)
    finally:
        os.close(fd)


def lock_file(fd, name):
    import fcntl
    descriptor = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600, dir_fd=fd)
    stream = os.fdopen(descriptor, "wb")
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_mode & 0o077:
            raise SyncError("Unsafe model synchronization lock.")
        deadline = time.monotonic() + 30
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return stream
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise SyncError("Model synchronization lock timed out.")
                time.sleep(0.05)
    except BaseException:
        stream.close()
        raise


def publish(fd, name, content, no_replace=False):
    # Refuse even dangling symlinks or special files at the destination.
    read_at(fd, name)
    tmp = ".model-sync-" + uuid.uuid4().hex
    out = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=fd)
    try:
        with os.fdopen(out, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        if no_replace:
            # Native OAuth writers do not participate in our lock. linkat's
            # no-replace publication is the actual concurrency guard.
            os.link(tmp, name, src_dir_fd=fd, dst_dir_fd=fd, follow_symlinks=False)
        else:
            os.replace(tmp, name, src_dir_fd=fd, dst_dir_fd=fd)
        os.fsync(fd)
    finally:
        try:
            os.unlink(tmp, dir_fd=fd)
        except FileNotFoundError:
            pass


def project_oauth(name, store):
    if not isinstance(store, dict):
        raise SyncError("Malformed local provider authentication.")
    accounts_key, active_key, fields = OAUTH[name]
    accounts = store.get(accounts_key, [])
    if name == "auth.json" and not accounts and store.get("anthropic"):
        old = store["anthropic"]
        accounts = [dict(old, label="claude-otter")]
    if not isinstance(accounts, list):
        raise SyncError("Malformed local provider accounts.")
    result = []
    labels = set()
    for account in accounts:
        if not isinstance(account, dict):
            raise SyncError("Malformed local provider account.")
        projected = {k: account[k] for k in fields if k in account}
        required = ("access_token", "refresh_token") if name == "openai-auth.json" else ("access", "refresh")
        if not all(isinstance(projected.get(k), str) and projected[k].strip() for k in ("label",) + required):
            raise SyncError("Local provider account is incomplete.")
        expiry_key = "expires_at" if name == "openai-auth.json" else "expires"
        if name == "auth.json" and expiry_key not in projected:
            raise SyncError("Local Claude authentication expiry is missing.")
        if projected.get(expiry_key) is not None and (type(projected[expiry_key]) is not int or not 0 < projected[expiry_key] < 2**63):
            raise SyncError("Invalid provider authentication expiry.")
        for key in ("id_token", "account_id", "subscription_type"):
            if projected.get(key) is not None and not isinstance(projected[key], str):
                raise SyncError("Invalid provider account metadata.")
        if "scopes" in projected and (not isinstance(projected["scopes"], list) or not all(isinstance(v, str) for v in projected["scopes"])):
            raise SyncError("Invalid provider account scopes.")
        if projected["label"] in labels:
            raise SyncError("Duplicate local provider account labels.")
        labels.add(projected["label"])
        result.append(projected)
    if not result:
        return None
    active = store.get(active_key) or result[0]["label"]
    if active not in labels:
        raise SyncError("Invalid active provider account.")
    return {accounts_key: result, active_key: active}


def collect(home, environ=None):
    """Project values, never transport entire source files or environment maps."""
    import tomllib
    environ = os.environ if environ is None else environ
    home = Path(home)
    data_dir = Path(environ.get("JCODE_HOME", home / ".jcode"))
    config_dir = (data_dir / "config/jcode" if environ.get("JCODE_HOME") else
                  Path(environ.get("XDG_CONFIG_HOME", home / ".config")) / "jcode")
    result = {"version": 1, "oauth": {}, "env": {}, "settings": {}}
    for name in OAUTH:
        raw = read_path(data_dir / name)
        if raw is not None:
            projected = project_oauth(name, json.loads(raw))
            if projected:
                result["oauth"][name] = projected
    raw = read_path(data_dir / "gemini_oauth.json")
    if raw is not None:
        store = json.loads(raw)
        fields = ("access_token", "refresh_token", "expires_at")
        if not all(k in store for k in fields):
            raise SyncError("Local Gemini model authentication is incomplete.")
        result["oauth"]["gemini_oauth.json"] = {k: store[k] for k in fields}
    for name, keys in API_KEYS.items():
        raw = read_path(config_dir / name, b"").decode()
        saved = {}
        for line in raw.splitlines():
            key, sep, value = line.partition("=")
            if sep and key in keys:
                saved.setdefault(key, value.strip().strip("\"'"))
        values = {}
        for key in keys:
            value = (environ.get(key) or "").strip() or (saved.get(key) or "").strip()
            if value:
                if not isinstance(value, str) or any(c in value for c in "\r\n\0"):
                    raise SyncError("Invalid model API credential value.")
                values[key] = value
        if values:
            result["env"][name] = values
    raw = read_path(data_dir / "config.toml", b"")
    config = tomllib.loads(raw.decode())
    for section, fields in (("provider", PROVIDER_FIELDS), ("agents", AGENT_FIELDS)):
        result["settings"][section] = {k: v for k, v in config.get(section, {}).items() if k in fields}
    for section, fields in (("provider", PROVIDER_FIELDS), ("agents", AGENT_FIELDS)):
        for field in fields:
            env_key = {"default_model": "JCODE_MODEL", "default_provider": "JCODE_PROVIDER"}.get(field, "JCODE_" + field.upper())
            if env_key not in environ:
                continue
            value = environ[env_key].strip()
            if field == "cross_provider_failover":
                aliases = {"auto": "countdown", "automatic": "countdown", "off": "manual", "false": "manual", "disabled": "manual", "none": "manual"}
                value = aliases.get(value.lower(), value.lower())
                if value not in ("manual", "countdown"):
                    continue
            elif field in BOOL_FIELDS:
                if value.lower() not in ("1", "0", "true", "false", "yes", "no", "on", "off"):
                    continue
                value = value.lower() in ("1", "true", "yes", "on")
            elif field in INT_FIELDS:
                if not value.isdigit() or int(value) <= 0:
                    continue
                value = int(value)
            elif field == "model_picker_providers":
                continue
            elif not value:
                if section == "agents":
                    result["settings"][section].pop(field, None)
                continue
            result["settings"][section][field] = value
    project = environ.get("GOOGLE_CLOUD_PROJECT") or environ.get("GOOGLE_CLOUD_PROJECT_ID")
    if project and project.strip():
        result["settings"]["provider"]["gemini_project"] = project.strip()
    # Named providers can refer to arbitrary env/files/headers. Do not silently
    # exfiltrate those broader secrets under a model-only permission.
    if config.get("providers"):
        raise SyncError("Named provider profiles need an explicit model-sync allowlist before cloud spawn.")
    if len(canonical(result)) > LIMIT:
        raise SyncError("Model sync snapshot exceeds size limit.")
    return result


def toml_value(value):
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return json.dumps(value, allow_nan=False)
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(v) for v in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(json.dumps(k, ensure_ascii=False) + " = " + toml_value(v) for k, v in value.items()) + " }"
    import datetime
    if isinstance(value, (datetime.datetime, datetime.date, datetime.time)):
        return value.isoformat()
    raise SyncError("Unsupported model configuration value.")


def toml_document(config):
    # Inline tables allow lossless semantic preservation of arbitrary remote
    # configuration, including arrays of tables, without copying local hooks.
    return ("\n".join(json.dumps(k, ensure_ascii=False) + " = " + toml_value(v) for k, v in config.items()) + "\n").encode()


def validate(payload):
    if not isinstance(payload, dict) or set(payload) != {"version", "oauth", "env", "settings"} or payload["version"] != 1:
        raise SyncError("Invalid model sync envelope.")
    for name, value in payload["oauth"].items():
        if name in OAUTH:
            if project_oauth(name, value) != value:
                raise SyncError("Non-provider fields in model authentication.")
        elif name == "gemini_oauth.json":
            if (not isinstance(value, dict) or set(value) != {"access_token", "refresh_token", "expires_at"}
                    or not all(isinstance(value[k], str) and value[k].strip() for k in ("access_token", "refresh_token"))
                    or type(value["expires_at"]) is not int or not 0 < value["expires_at"] < 2**63):
                raise SyncError("Invalid Gemini model authentication.")
        else:
            raise SyncError("Unapproved model authentication destination.")
    for name, values in payload["env"].items():
        if name not in API_KEYS or not isinstance(values, dict) or not set(values) <= set(API_KEYS[name]):
            raise SyncError("Unapproved model API credential destination.")
        if not all(isinstance(v, str) and v and not any(c in v for c in "\n\r\0") for v in values.values()):
            raise SyncError("Invalid model API credential value.")
    if set(payload["settings"]) != {"provider", "agents"}:
        raise SyncError("Unapproved model configuration section.")
    for section, fields in (("provider", PROVIDER_FIELDS), ("agents", AGENT_FIELDS)):
        if not set(payload["settings"][section]) <= fields:
            raise SyncError("Unapproved model configuration field.")
        for key, value in payload["settings"][section].items():
            valid = (type(value) is bool if key in BOOL_FIELDS else
                     type(value) is int and 0 <= value < 2**64 if key in INT_FIELDS else
                     isinstance(value, list) and all(isinstance(v, str) for v in value) if key == "model_picker_providers" else
                     isinstance(value, str))
            if not valid:
                raise SyncError("Invalid model configuration value type.")
            if key == "max_retries" and value >= 2**32:
                raise SyncError("Invalid model retry limit.")
            if key == "cross_provider_failover" and value not in ("manual", "countdown", "off", "false", "disabled", "none"):
                raise SyncError("Invalid model failover policy.")


def apply_snapshot(home, payload, refresh=None):
    """Remote receiver. Native stores, per-input dedup, serialized publishers."""
    import tomllib
    validate(payload)
    data_fd = directory(Path(home) / ".jcode", create=True)
    config_fd = directory(Path(home) / ".config/jcode", create=True)
    try:
        with lock_file(data_fd, ".cloud-model-sync.lock"):
            ledger = json.loads(read_at(data_fd, "cloud-model-sync-state.json", b"{}"))
            staged = []
            for name, value in payload["oauth"].items():
                checksum = digest(value)
                if ledger.get(name) == checksum and read_at(data_fd, name) is not None:
                    continue  # Keep a remote refresh, not a stale local token.
                existing_raw = read_at(data_fd, name)
                if existing_raw is not None:
                    existing = json.loads(existing_raw)
                    projected = project_oauth(name, existing) if name in OAUTH else {
                        k: existing[k] for k in ("access_token", "refresh_token", "expires_at") if k in existing}
                    if projected != value:
                        raise OAuthConflict("Cloud OAuth differs from local login. Existing cloud credentials were preserved; synchronize this account explicitly before spawning.")
                    # Equal store is already usable. Never replace even equal
                    # bytes: a provider may refresh immediately after this read.
                else:
                    staged.append((data_fd, name, canonical(value)))
                ledger[name] = checksum
            for name, values in payload["env"].items():
                checksum = digest(values)
                if ledger.get(name) == checksum and read_at(config_fd, name) is not None:
                    continue
                before = read_at(config_fd, name, b"").decode()
                lines = [line for line in before.splitlines() if line.partition("=")[0] not in values]
                lines.extend(k + "=" + v for k, v in values.items())
                staged.append((config_fd, name, ("\n".join(lines) + "\n").encode()))
                ledger[name] = checksum
            checksum = digest(payload["settings"])
            if ledger.get("settings") != checksum or read_at(data_fd, "config.toml") is None:
                config = tomllib.loads(read_at(data_fd, "config.toml", b"").decode())
                for section, fields in (("provider", PROVIDER_FIELDS), ("agents", AGENT_FIELDS)):
                    dest = config.setdefault(section, {})
                    for key in fields:
                        dest.pop(key, None)
                    dest.update(payload["settings"][section])
                staged.append((data_fd, "config.toml", toml_document(config)))
                ledger["settings"] = checksum
            # Validate every destination before publishing any changes.
            for fd, name, _ in staged:
                read_at(fd, name)
            for fd, name, content in staged:
                publish(fd, name, content, no_replace=name in payload["oauth"])
            if refresh is not None and ledger.get("runtime") != digest(payload):
                refresh(payload)
                ledger["runtime"] = digest(payload)
            publish(data_fd, "cloud-model-sync-state.json", canonical(ledger))
            return len(staged)
    finally:
        os.close(config_fd)
        os.close(data_fd)


def refresh_daemon(payload, socket_paths=None):
    """Use native secret-free auth notification, never restart existing sessions.

    Done is only enqueue acknowledgement. Wait for catalog_activity completion
    on this dedicated no-prompt connection before notifying the next provider.
    """
    import socket
    import tempfile
    providers = set()
    for name in payload["oauth"]:
        providers.add({"openai-auth.json": "openai", "auth.json": "anthropic",
                       "gemini_oauth.json": "gemini"}[name])
    for name in payload["env"]:
        providers.add({"openai.env": "openai-api", "anthropic.env": "anthropic"}.get(name, name[:-4]))
    if not providers:
        return
    if socket_paths is None:
        uid = os.getuid()
        socket_paths = [Path("/run/user") / str(uid) / "jcode.sock",
                        Path(tempfile.gettempdir()) / ("jcode-" + str(uid)) / "jcode.sock"]
        if os.environ.get("JCODE_SOCKET"):
            socket_paths = [Path(os.environ["JCODE_SOCKET"])]
    deadline = time.monotonic() + 30
    for path in socket_paths:
        path = Path(path)
        try:
            info = path.lstat()
        except FileNotFoundError:
            continue  # Fresh boot: daemon will read native stores at startup.
        if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid():
            raise SyncError("Unsafe cloud daemon socket.")
        parent = directory(path.parent)
        os.close(parent)
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(max(0.1, deadline - time.monotonic()))
            client.connect(str(path))
            with client.makefile("rb") as stream:
                for request_id, provider in enumerate(sorted(providers), 1):
                    client.sendall(canonical({"type": "notify_auth_changed", "id": request_id,
                                              "provider": provider, "prefer_strongest": False}) + b"\n")
                    while True:
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            raise SyncError("Cloud model refresh timed out.")
                        client.settimeout(remaining)
                        raw = stream.readline(LIMIT + 1)
                        if not raw or len(raw) > LIMIT:
                            raise SyncError("Cloud model refresh connection failed.")
                        event = json.loads(raw)
                        if event.get("type") == "error":
                            raise SyncError("Cloud model refresh was rejected.")
                        if (event.get("type") == "notification"
                                and event.get("notification_type", {}).get("scope") == "catalog_activity"
                                and event.get("message", "").startswith(("**Model ready:**", "**Model access refreshed**"))):
                            break


def receiver_source():
    """Carry the local standard-library TOML parser in memory for Python 3.9.

    Only public Python source is placed in argv. Provider data is never part of
    this string. Module copyright headers remain intact. No remote installation.
    """
    import ast
    import tomllib
    root = Path(tomllib.__file__).parent
    modules = {}
    for name in ("_types", "_re", "_parser", "__init__"):
        source = (root / (name + ".py")).read_text()
        ast.parse(source, feature_version=(3, 9))
        modules[name] = source
    bootstrap = "import sys, types\n"
    bootstrap += "_toml = types.ModuleType('tomllib'); _toml.__path__ = []; sys.modules['tomllib'] = _toml\n"
    for name, source in modules.items():
        full_name = "tomllib" if name == "__init__" else "tomllib." + name
        bootstrap += "_name = " + repr(full_name) + "\n"
        bootstrap += "_mod = sys.modules.get(_name) or types.ModuleType(_name); _mod.__package__ = 'tomllib'; sys.modules[_name] = _mod\n"
        bootstrap += "exec(compile(" + repr(source) + ", '<model-sync-tomllib>', 'exec'), _mod.__dict__)\n"
    return bootstrap + Path(__file__).read_text()


def sync_personal_alpha(config, *, home=None, environ=None, runner=None, verify=None, now=None):
    """Default wake hook. Cache hits do no AWS or SSH and expire after 30s.

    Caller must perform ordinary VM readiness separately. Actual transfer also
    verifies account and alpha instance ownership before touching the SSH alias.
    """
    home = Path.home() if home is None else Path(home)
    runner = subprocess.run if runner is None else runner
    now = time.time() if now is None else now
    try:
        snapshot = collect(home, environ)
        validate(snapshot)
        # Target/config/helper/pin changes invalidate even an otherwise warm hit.
        target = {k: config[k] for k in ("account_id", "instance_id", "profile", "region")}
        if not re.fullmatch(r"i-[0-9a-f]{8,17}", target["instance_id"]) or not re.fullmatch(r"[0-9]{12}", target["account_id"]):
            raise SyncError("Invalid personal-cloud target identity.")
        target["pin"] = hashlib.sha256(read_path(home / ".ssh/jcode_cloud_alpha_known_hosts", b"")).hexdigest()
        target["ssh"] = hashlib.sha256(read_path(home / ".ssh/config", b"")).hexdigest()
        target["code"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        key = digest({"target": target, "snapshot": snapshot})
        fd = directory(home / ".jcode", create=True)
        try:
            with lock_file(fd, ".cloud-model-sync-local.lock"):
                cache = json.loads(read_at(fd, "cloud-model-sync-local.json", b"{}"))
                if cache.get("key") == key and 0 <= now - cache.get("at", 0) < CACHE_TTL:
                    return False
                if verify is None:
                    import control
                    control.identity(config)
                    remote = control.instance(config)
                    if remote["State"]["Name"] != "running":
                        raise SyncError("Cloud host must be running before model synchronization.")
                else:
                    verify(config)
                pinned = home / ".ssh/jcode_cloud_alpha_known_hosts"
                if not read_path(pinned, b""):
                    raise SyncError("Pinned personal-cloud host key is required for model synchronization.")
                source = receiver_source()
                command = "python3 -c " + shlex.quote(source) + " --receive"
                result = runner(["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8",
                                 "-o", "StrictHostKeyChecking=yes", "-o", "HostKeyAlias=" + HOST,
                                 "-o", "UserKnownHostsFile=" + str(pinned),
                                 "-o", "HostName=" + target["instance_id"], "-l", "ec2-user", HOST, command],
                                input=canonical(snapshot), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                timeout=45, check=False)
                if result.stdout == b"model-sync-oauth-conflict\n":
                    raise OAuthConflict("Cloud OAuth differs from local login. Existing cloud credentials were preserved; reconcile the account explicitly before spawning.")
                if result.returncode != 0 or result.stdout != b"model-sync-ok\n":
                    raise SyncError("Cloud model synchronization failed. Existing sessions were not restarted.")
                publish(fd, "cloud-model-sync-local.json", canonical({"key": key, "at": now}))
                return True
        finally:
            os.close(fd)
    except SyncError:
        raise
    except Exception:
        # JSON/TOML parser errors and subprocess errors may contain credentials.
        raise SyncError("Model synchronization could not safely read or transfer provider settings.") from None


def receive():
    try:
        raw = sys.stdin.buffer.read(LIMIT + 1)
        if len(raw) > LIMIT:
            raise SyncError("Oversized model sync input.")
        apply_snapshot(Path.home(), json.loads(raw), refresh=refresh_daemon)
        sys.stdout.write("model-sync-ok\n")
    except OAuthConflict:
        sys.stdout.write("model-sync-oauth-conflict\n")
        return 2
    except Exception:
        sys.stderr.write("Model synchronization failed safely.\n")
        return 1
    return 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--receive"]:
        sys.exit(receive())
    sys.stderr.write("Use the personal-cloud wake or sync-models helper.\n")
    sys.exit(2)
