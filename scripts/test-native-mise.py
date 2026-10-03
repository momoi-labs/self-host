#!/usr/bin/env python3
"""Exercise native mise recipes on a disposable Linux Host with the real daemon.

Run as root with Python 3, curl and native supervision. SELF_HOST_TEST_URL must
be the isolated loopback API; SELF_HOST_TEST_KEY holds its key. The daemon uses
the official mise installer and downloads Node 22 and 24. If supervision uses
a fixture path, set SELF_HOST_TEST_NATIVE_ROOT to read its protected setup log.

Successful runs remove only their own Applications and homes. Failures retain
synthetic fixtures for diagnosis. Never run against a production daemon.
"""

import copy
import json
import os
from pathlib import Path
import pwd
import shlex
import shutil
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE = os.environ["SELF_HOST_TEST_URL"].rstrip("/")
KEY = os.environ["SELF_HOST_TEST_KEY"]
ADDRESS = urllib.parse.urlsplit(BASE)
assert ADDRESS.scheme in {"http", "https"} and ADDRESS.hostname in {"127.0.0.1", "localhost", "::1"}, "Use the disposable Host's loopback API"
assert not ADDRESS.username and not ADDRESS.password and not ADDRESS.path and not ADDRESS.query and not ADDRESS.fragment, "Use an API origin without credentials or a path"
assert sys.platform == "linux" and os.geteuid() == 0, "Run as root on a disposable Linux Host"
NATIVE_ROOT = Path(os.environ.get("SELF_HOST_TEST_NATIVE_ROOT", "/var/lib/self-host/native"))
assert NATIVE_ROOT.is_absolute(), "The fixture supervision root must be absolute"
PREFIX = "native-mise-test-" + uuid.uuid4().hex[:8]
SECRET = "synthetic-setup-secret-" + uuid.uuid4().hex


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return None


# Neither proxy environment settings nor redirects may forward the API key.
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def open_request(path, method="GET", body=None, expected=200, timeout=60):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json", "Authorization": "Bearer " + KEY}
    try:
        response = HTTP.open(urllib.request.Request(BASE + path, data=data, method=method, headers=headers), timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    assert response.status == expected, f"{method} {path}: expected {expected}, got {response.status}"
    return response


def request(path, method="GET", body=None, expected=200, timeout=60):
    with open_request(path, method, body, expected, timeout) as response:
        raw = response.read()
    return json.loads(raw) if raw else None


def wait(read, ready, message, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.25)
    raise AssertionError(message)


def task(accepted, failed=False):
    deadline = time.monotonic() + 900
    while time.monotonic() < deadline:
        events = request("/events", timeout=min(15, max(1, deadline - time.monotonic())))
        event = next((item for item in events if item["id"] == accepted["task_id"]), None)
        if event and event["status"] in {"completed", "failed"}:
            assert event["status"] == ("failed" if failed else "completed"), "Unexpected Task outcome; inspect the synthetic fixture's logs"
            assert SECRET not in json.dumps(event), "Task output exposed the synthetic setup Variable"
            return event
        time.sleep(0.5)
    raise AssertionError("Task did not finish within 15 minutes")


PROBE = r'''
const fs = require("node:fs");
const path = require("node:path");
const home = process.env.HOME;
fs.appendFileSync(path.join(home, "launches"), String(process.pid) + "\n", {mode: 0o600});
let other = null;
if (process.env.OTHER_CONFIG) {
  try { fs.readFileSync(process.env.OTHER_CONFIG); other = "readable"; }
  catch (error) { other = error.code; }
}
const evidence = {
  uid: process.getuid(), euid: process.geteuid(), gid: process.getgid(), groups: process.getgroups(),
  pid: process.pid, version: process.versions.node, executable: fs.realpathSync(process.execPath),
  home, cwd: process.cwd(), cgroup: fs.readFileSync("/proc/self/cgroup", "utf8"),
  paths: Object.fromEntries(["MISE_CONFIG_DIR", "MISE_GLOBAL_CONFIG_FILE", "MISE_DATA_DIR", "MISE_CACHE_DIR", "MISE_STATE_DIR", "MISE_TMP_DIR", "MISE_SYSTEM_CONFIG_DIR", "MISE_INSTALL_PATH", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "CARGO_HOME", "RUSTUP_HOME"].map(key => [key, process.env[key]])),
  auto_install: process.env.MISE_AUTO_INSTALL, other_config: other,
  owns_config: fs.readFileSync(process.env.MISE_GLOBAL_CONFIG_FILE, "utf8").includes("[tools]"),
  setting: process.env.TEST_SETTING || null
};
fs.writeFileSync(path.join(home, "probe.json.tmp"), JSON.stringify(evidence), {mode: 0o600});
fs.renameSync(path.join(home, "probe.json.tmp"), path.join(home, "probe.json"));
console.log("native-mise-main-ready");
setInterval(() => {}, 60000);
'''

SETUP = r'''
const fs = require("node:fs");
const path = require("node:path");
const home = process.env.HOME;
fs.appendFileSync(path.join(home, "setup-count"), "run\n", {mode: 0o600});
fs.writeFileSync(path.join(home, "setup.json"), JSON.stringify({uid: process.getuid(), euid: process.geteuid(), gid: process.getgid(), version: process.versions.node, executable: fs.realpathSync(process.execPath), home, cwd: process.cwd(), cgroup: fs.readFileSync("/proc/self/cgroup", "utf8")}), {mode: 0o600});
'''.replace("\n", " ")


def recipe(major):
    return {
        "dependencies": [{"tool": "node", "version": str(major)}],
        "setup": ["mkdir -p app", "node -e " + shlex.quote(SETUP), 'printf "setup-log-check:%s\\n" "$SETUP_SECRET"'],
    }


def record(app):
    return request(f"/apps/id/{app['id']}")


def evidence(app, filename="probe.json"):
    path = app["home"] / filename
    return json.loads(path.read_text()) if path.exists() else {}


def count(app, filename):
    path = app["home"] / filename
    return len(path.read_text().splitlines()) if path.exists() else 0


def group_of(value, identifier):
    unified = [line[3:] for line in value.splitlines() if line.startswith("0::")]
    assert len(unified) == 1 and Path(unified[0]).name == f"sf-app-{identifier}", "Process did not join its Application cgroup"
    return Path("/sys/fs/cgroup") / unified[0].lstrip("/")


def assert_identity(app, value, major, setup=False):
    identity, home = app["identity"], app["home"]
    assert value["uid"] == value["euid"] == identity.pw_uid != 0, "Native command used the wrong uid"
    assert value["gid"] == identity.pw_gid != 0, "Native command used the wrong gid"
    assert value["home"] == str(home), "Native command used another home"
    assert value["cwd"] == str(home if setup else home / "app"), "Setup or main used the wrong working directory"
    assert value["version"].split(".")[0] == str(major), "The configured Node major was not installed"
    executable = Path(value["executable"])
    assert executable.is_relative_to(home / ".self-host/mise/data"), "Node came from a shared installation"
    group = group_of(value["cgroup"], app["id"])
    if not setup:
        assert set(value["groups"]) <= {identity.pw_gid}, "Native command inherited supplementary groups"
        assert value["auto_install"] == "0" and value["owns_config"], "The main process lost its isolated mise configuration"
        assert str(value["pid"]) in (group / "cgroup.procs").read_text().splitlines(), "Main process escaped the Application cgroup"
    return group


def assert_private_paths(app, observed):
    home, identity = app["home"], app["identity"]
    assert home.is_absolute() and home.name == app["id"] and home.resolve() == home, "Unexpected fixture home"
    assert home.stat().st_uid == identity.pw_uid and home.stat().st_mode & 0o777 == 0o750, "Application home lost its dedicated-account permissions"
    root = home / ".self-host/mise"
    for path in [home / ".self-host", root, *map(Path, observed["paths"].values())]:
        assert path.exists() and path.resolve().is_relative_to(home), "A mise path leaves the Application home"
        assert path.stat().st_uid == identity.pw_uid, "A mise path is not owned by the Application Account"
    for relative in ["config", "data", "cache", "state", "tmp", "system"]:
        assert (root / relative).stat().st_mode & 0o077 == 0, "A private mise directory is accessible by another account"
    config = root / "config/config.toml"
    assert config.is_file() and not config.is_symlink() and config.stat().st_mode & 0o077 == 0, "The mise configuration is not private"


def assert_logs_redacted(app):
    path = NATIVE_ROOT / "logs" / app["id"] / "recipe.log"
    assert path.is_file(), "Preparation log is absent from the fixture supervision root"
    stored = path.read_bytes()
    assert SECRET.encode() not in stored, "Stored preparation log exposed the synthetic Variable"
    assert b"setup-log-check:[redacted]" in stored, "Stored preparation log did not contain the redacted setup evidence"
    received = bytearray()
    with open_request(f"/apps/id/{app['id']}/logs", timeout=10) as response:
        while len(received) < 512 * 1024:
            line = response.readline(65536)
            assert line, "The setup log stream ended before its completion marker"
            received.extend(line)
            if b"Native environment ready." in line:
                break
        else:
            raise AssertionError("The setup log stream exceeded its fixture bound")
    assert SECRET.encode() not in received, "API preparation logs exposed the synthetic Variable"
    assert b"setup-log-check:[redacted]" in received, "API logs omitted the redacted setup evidence"


def create(major):
    name = f"{PREFIX}-node-{major}"
    command = ["node", "-e", PROBE]
    accepted = request("/apps", "POST", {
        "name": name,
        "runtime": {"kind": "native", "account": "", "command": command, "working_dir": "app", "recipe": recipe(major), "limits": {"cpu_percent": 100, "memory_bytes": 536870912, "max_tasks": 128}},
        "publication": {"kind": "unpublished"},
        "environment": {"SETUP_SECRET": SECRET},
    }, 202)
    task(accepted)
    app = {"id": accepted["id"], "name": name}
    current = record(app)
    assert current["runtime"]["command"] == command, "The API replaced the raw Application argv with its mise wrapper"
    assert current["runtime"]["recipe"] == recipe(major), "The API changed the configured recipe"
    assert current["runtime"]["account"] == f"sf-app-{app['id']}"
    app["identity"] = pwd.getpwnam(current["runtime"]["account"])
    app["home"] = Path(app["identity"].pw_dir)
    observed = wait(lambda: evidence(app), bool, "Native Application did not write its probe")
    group = assert_identity(app, observed, major)
    setup = evidence(app, "setup.json")
    assert_identity(app, setup, major, setup=True)
    assert setup["cgroup"] == observed["cgroup"], "Setup and main used different cgroups"
    assert count(app, "setup-count") == 1 and count(app, "launches") == 1, "Initial preparation or main ran more than once"
    assert_private_paths(app, observed)
    assert_logs_redacted(app)
    app["group"] = group
    return app


def action(app, verb, failed=False):
    task(request(f"/apps/id/{app['id']}/{verb}", "POST", expected=202), failed=failed)


def assert_stopped(app, launches):
    current = record(app)
    assert current["status"] == "stopped" and current["services"][0]["state"] == "exited", "The Application lost its stopped intent"
    assert count(app, "launches") == launches, "Preparing a stopped Application launched its main command"
    procs = app["group"] / "cgroup.procs"
    assert not procs.exists() or not procs.read_text().strip(), "A stopped Application retained child processes"


def variable(app, name, value):
    previous = evidence(app)["pid"]
    prepared = count(app, "setup-count")
    task(request(f"/apps/{app['name']}/env", "POST", {"key": name, "value": value}, 202))
    observed = wait(lambda: evidence(app), lambda item: item.get("pid") != previous, "Changing a Variable did not restart the main command")
    assert count(app, "setup-count") == prepared, "Changing a Variable reran the environment recipe"
    return observed


def remove(app):
    current = record(app)
    assert current["id"] == app["id"] and current["name"] == app["name"] and app["name"].startswith(PREFIX + "-"), "Refusing to remove an Application outside this run"
    task(request(f"/apps/{app['name']}", "DELETE", expected=202))
    request(f"/apps/id/{app['id']}", expected=404)
    home = app["home"]
    assert home.resolve() == home and home.name == app["id"] and not home.is_symlink(), "Refusing to remove an unexpected retained home"
    assert home.stat().st_uid == 0 and home.stat().st_mode & 0o777 == 0o700, "Removal did not protect retained Application data"
    shutil.rmtree(home)


def main():
    print("Disposable fixture prefix:", PREFIX, flush=True)
    print("Installing Node 22 and 24 under separate Application Accounts.", flush=True)
    first, second = create(22), create(24)
    assert first["home"] != second["home"] and first["group"] != second["group"], "Applications share their home or cgroup"
    assert first["identity"].pw_uid != second["identity"].pw_uid, "Applications share their execution account"
    for app, other, major in [(first, second, 22), (second, first, 24)]:
        observed = variable(app, "OTHER_CONFIG", str(other["home"] / ".self-host/mise/config/config.toml"))
        assert observed["other_config"] in {"EACCES", "EPERM"}, "One Application could read another Application's configuration"
        assert_identity(app, observed, major)

    print("Checking stopped updates, unchanged recipes and preparation failure.", flush=True)
    action(first, "stop")
    launches, prepared = count(first, "launches"), count(first, "setup-count")
    assert_stopped(first, launches)
    updated = copy.deepcopy(record(first)["runtime"])
    updated["recipe"] = recipe(24)
    task(request(f"/apps/id/{first['id']}", "PUT", {"runtime": updated}, 202))
    assert_stopped(first, launches)
    assert count(first, "setup-count") == prepared + 1, "Changing a stopped recipe did not prepare its new environment"
    assert_identity(first, evidence(first, "setup.json"), 24, setup=True)
    action(first, "start")
    observed = wait(lambda: evidence(first), lambda item: item.get("version", "").startswith("24."), "Start did not use the prepared Node version")
    assert_identity(first, observed, 24)
    assert_private_paths(first, observed)
    assert count(first, "setup-count") == prepared + 1, "Start reran the prepared recipe"
    observed = variable(first, "TEST_SETTING", "changed")
    assert observed["setting"] == "changed"
    assert_identity(first, observed, 24)

    action(first, "stop")
    launches = count(first, "launches")
    previous = evidence(first)["pid"]
    broken = copy.deepcopy(updated)
    broken["recipe"]["setup"].append("printf 'synthetic setup failure\\n'; exit 19")
    task(request(f"/apps/id/{first['id']}", "PUT", {"runtime": broken}, 202), failed=True)
    assert record(first)["status"] == "failed", "Failed preparation was reported as successful"
    assert count(first, "launches") == launches, "Failed preparation launched the main command"
    failed_preparations = count(first, "setup-count")
    action(first, "start", failed=True)
    assert count(first, "launches") == launches, "Start bypassed failed environment preparation"
    assert count(first, "setup-count") == failed_preparations, "Start retried setup without an explicit reapply"
    task(request(f"/apps/id/{first['id']}", "PUT", {"runtime": updated}, 202))
    assert_stopped(first, launches)
    assert count(first, "setup-count") == failed_preparations + 1, "Repair did not reapply the valid recipe"
    action(first, "start")
    observed = wait(lambda: evidence(first), lambda item: item.get("pid") != previous, "Reapplying the valid recipe did not repair the Application")
    assert record(first)["status"] == "running"
    assert count(first, "setup-count") == failed_preparations + 1, "Start repeated the repaired recipe"
    assert count(first, "launches") == launches + 1, "Repair did not launch the main command exactly once"
    assert_identity(first, observed, 24)
    assert_logs_redacted(first)
    assert record(first)["runtime"]["command"] == ["node", "-e", PROBE], "Reapplying changed the raw Application command"

    for app in [first, second]:
        remove(app)
    print("PASS: private Node versions, non-root setup, separate homes/cgroups, stopped recipe updates, cached preparation, failed-setup guard, redacted logs and fixture cleanup")


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        print("FAIL: retained synthetic fixtures with prefix", PREFIX, flush=True)
        raise
