#!/usr/bin/env python3
"""Exercise P2 and N4 on a disposable Linux Host with the real daemon.

Run as root with postgresql-client, Python 3, Docker and native supervision.
SELF_HOST_TEST_URL and SELF_HOST_TEST_KEY identify the loopback fixture API.
The request/task/connection helpers follow test-managed-postgres.py. Failures
retain synthetic fixtures for diagnosis. Successful runs remove only their
own Applications, database volume and native home. Never use production.
"""

import json
import os
from pathlib import Path
import pwd
import shutil
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE = os.environ["SELF_HOST_TEST_URL"].rstrip("/")
KEY = os.environ["SELF_HOST_TEST_KEY"]
assert urllib.parse.urlsplit(BASE).hostname in {"127.0.0.1", "localhost", "::1"}, "Use the disposable Host's loopback API"
assert os.geteuid() == 0 and Path("/usr/bin/psql").is_file(), "Run as root on the disposable Host after installing postgresql-client"
PREFIX = "native-pg-test-" + uuid.uuid4().hex[:8]


def request(path, method="GET", body=None, expected=200, authenticated=True):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"}
    if authenticated:
        headers["Authorization"] = "Bearer " + KEY
    try:
        response = urllib.request.urlopen(urllib.request.Request(BASE + path, data=data, method=method, headers=headers), timeout=60)
    except urllib.error.HTTPError as error:
        response = error
    assert response.status == expected, f"{method} {path}: expected {expected}, got {response.status}"
    raw = response.read()
    return json.loads(raw) if raw else None


def wait(read, ready, message, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = read()
        if ready(value):
            return value
        time.sleep(0.25)
    raise AssertionError(message)


def task(accepted):
    event = wait(lambda: next((e for e in request("/events") if e["id"] == accepted["task_id"]), None), lambda e: e and e["status"] in {"completed", "failed"}, "Task did not finish", 240)
    assert event["status"] == "completed", "Unexpected Task outcome; inspect the synthetic fixture's Last run"


def connection(database, identifier, status):
    def read():
        current = next(c for c in request(f"/databases/{database}")["connections"] if c["id"] == identifier)
        assert current["status"] != "failed", "Connection failed"
        return current
    return wait(read, lambda c: c["status"] == status, "Connection did not settle")


def run(arguments, data=None, environment=None, success=True):
    result = subprocess.run(arguments, input=data, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
    assert (result.returncode == 0) == success, "Unexpected PostgreSQL or Docker outcome"
    return result.stdout.decode().strip()


def old_login(url, provider=None):
    parsed = urllib.parse.urlsplit(url)
    password = urllib.parse.unquote(parsed.password)
    user = urllib.parse.unquote(parsed.username)
    database = urllib.parse.unquote(parsed.path.lstrip("/"))
    if provider:
        # Force TCP while the provider is healthy. A removed Host port is not
        # evidence that PostgreSQL revoked the credential itself.
        shell = 'IFS= read -r PGPASSWORD; export PGPASSWORD PGCONNECT_TIMEOUT=5; exec psql --no-psqlrc --no-password --quiet --tuples-only --no-align --set ON_ERROR_STOP=1 --host 127.0.0.1 --port 5432 --username "$1" --dbname "$2"'
        result = subprocess.run(["docker", "exec", "--user", "postgres", "-i", provider, "sh", "-c", shell, "query", user, database], input=(password + "\nSELECT 1;\n").encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        assert result.returncode == 2 and any(reason in result.stderr for reason in [b"password authentication failed", b"is not permitted to log in"]), "PostgreSQL did not reject the revoked login"
        return
    return run(["/usr/bin/psql", "--no-psqlrc", "--no-password", "--quiet", "--host", parsed.hostname, "--port", str(parsed.port), "--username", user, "--dbname", database], data=b"SELECT 1;\n", environment={"PATH": "/usr/bin:/bin", "PGPASSWORD": password, "PGCONNECT_TIMEOUT": "5"}, success=False)


# Only the Application reads DATABASE_URL. Neither its command nor its
# evidence file contains the URL or password.
WORKER = r'''
import json, os, pathlib, subprocess, time, urllib.parse
with open("launches", "a") as output:
    output.write(str(os.getpid()) + "\n")
evidence = {"uid": os.getuid(), "euid": os.geteuid(), "gid": os.getgid(), "groups": os.getgroups(), "pid": os.getpid(), "cgroup": pathlib.Path("/proc/self/cgroup").read_text(), "has_database_url": bool(os.environ.get("DATABASE_URL"))}
assert evidence["uid"] != 0 and evidence["euid"] != 0
if evidence["has_database_url"]:
    parsed = urllib.parse.urlsplit(os.environ["DATABASE_URL"])
    assert parsed.hostname == "127.0.0.1"
    environment = {"PATH": "/usr/bin:/bin", "PGPASSWORD": urllib.parse.unquote(parsed.password), "PGCONNECT_TIMEOUT": "5"}
    sql = "SELECT json_build_object('role', current_user, 'database', current_database(), 'superuser', rolsuper, 'create_role', rolcreaterole, 'create_database', rolcreatedb) FROM pg_roles WHERE rolname = current_user;"
    result = subprocess.run(["/usr/bin/psql", "--no-psqlrc", "--no-password", "--quiet", "--tuples-only", "--no-align", "--set", "ON_ERROR_STOP=1", "--host", parsed.hostname, "--port", str(parsed.port), "--username", urllib.parse.unquote(parsed.username), "--dbname", urllib.parse.unquote(parsed.path.lstrip("/"))], input=sql, env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15)
    evidence["query_exit"] = result.returncode
    if result.returncode == 0:
        evidence["query"] = json.loads(result.stdout)
pathlib.Path("probe.json.tmp").write_text(json.dumps(evidence))
os.replace("probe.json.tmp", "probe.json")
while True:
    time.sleep(60)
'''


def main():
    print("Disposable fixture prefix:", PREFIX, flush=True)
    created = request("/databases", "POST", {"name": PREFIX, "major": 17}, 202)
    task(created)
    db = created["id"]
    provider = f"sf-app-{db}-database"
    details = request(f"/databases/{db}")
    assert details["readiness"] == "healthy"
    assert not run(["docker", "port", provider]), "Provider published a port before a native connection existed"

    name = PREFIX + "-client"
    created = request("/apps", "POST", {"name": name, "runtime": {"kind": "native", "account": "", "command": ["/usr/bin/python3", "-c", WORKER], "limits": {"cpu_percent": 50, "memory_bytes": 134217728, "max_tasks": 32}}, "publication": {"kind": "unpublished"}}, 202)
    task(created)
    consumer = created["id"]
    account = request(f"/apps/id/{consumer}")["runtime"]["account"]
    assert account == f"sf-app-{consumer}"
    identity = pwd.getpwnam(account)
    assert identity.pw_uid != 0 and identity.pw_gid != 0
    home = Path(identity.pw_dir)
    assert home.is_absolute() and home.name == consumer and not home.is_symlink(), "Unexpected fixture home"

    def probe():
        return json.loads((home / "probe.json").read_text()) if (home / "probe.json").exists() else {}

    first = wait(probe, lambda p: bool(p), "Native consumer did not write identity evidence")
    assert first["uid"] == first["euid"] == identity.pw_uid
    assert first["gid"] == identity.pw_gid and set(first["groups"]) <= {identity.pw_gid}
    assert not first["has_database_url"]
    relative = next(line[3:] for line in first["cgroup"].splitlines() if line.startswith("0::"))
    group = Path("/sys/fs/cgroup") / relative.lstrip("/")
    assert group.name == f"sf-app-{consumer}"
    task(request(f"/apps/id/{consumer}/stop", "POST", expected=202))
    launches = (home / "launches").read_text()

    def stopped():
        record = request(f"/apps/id/{consumer}")
        assert record["status"] == "stopped" and record["services"][0]["state"] == "exited", "Stopped intent changed"
        assert (home / "launches").read_text() == launches, "Database configuration launched a stopped consumer"
        assert not (group / "cgroup.procs").exists() or not (group / "cgroup.procs").read_text().strip(), "Stopped consumer still has processes"

    stopped()
    accepted = request(f"/databases/{db}/connections", "POST", {"consumer_application_id": consumer, "variable": "DATABASE_URL"}, 202)
    task(accepted)
    identifier = accepted["connection_id"]
    metadata = connection(db, identifier, "ready")
    stopped()
    request(f"/databases/{db}/connections/{identifier}/reveal", "POST", expected=401, authenticated=False)
    url = request(f"/databases/{db}/connections/{identifier}/reveal", "POST")["url"]
    parsed = urllib.parse.urlsplit(url)
    assert parsed.scheme == "postgresql" and parsed.hostname == "127.0.0.1" and parsed.port >= 1024
    assert parsed.username == metadata["role"] and parsed.path.lstrip("/") == metadata["database"]
    assert parsed.password and parsed.password not in json.dumps(request(f"/databases/{db}"))
    assert dict(request(f"/apps/{name}/env"))["DATABASE_URL"] == url
    assert run(["docker", "port", provider]).splitlines() == [f"5432/tcp -> 127.0.0.1:{parsed.port}"], "Provider published a non-loopback endpoint"

    task(request(f"/apps/id/{consumer}/start", "POST", expected=202))
    observed = wait(probe, lambda p: p.get("pid") != first["pid"] and p.get("has_database_url"), "Native query did not finish")
    assert observed["uid"] == observed["euid"] == identity.pw_uid and observed["query_exit"] == 0, "Native psql failed or used the wrong identity"
    assert observed["query"] == {"role": metadata["role"], "database": metadata["database"], "superuser": False, "create_role": False, "create_database": False}
    assert observed["cgroup"] == first["cgroup"]
    task(request(f"/apps/id/{consumer}/stop", "POST", expected=202))
    launches = (home / "launches").read_text()
    task(request(f"/databases/{db}/connections/{identifier}", "DELETE", expected=202))
    connection(db, identifier, "revoked")
    stopped()
    assert "DATABASE_URL" not in dict(request(f"/apps/{name}/env"))
    assert "DATABASE_URL" not in request(f"/apps/id/{consumer}/variable-names")
    assert request(f"/databases/{db}")["readiness"] == "healthy"
    old_login(url, provider)
    old_login(url)
    request(f"/databases/{db}/connections/{identifier}/reveal", "POST", expected=409)

    task(request(f"/apps/{name}", "DELETE", expected=202))
    request(f"/apps/id/{consumer}", expected=404)
    assert home.stat().st_uid == 0 and home.stat().st_mode & 0o777 == 0o700, "Native removal did not protect its retained home"
    # This newly allocated Application home holds only this test's evidence.
    # Never traverse a replaced path or remove a shared native-data directory.
    assert home.resolve() == home and home.name == consumer
    shutil.rmtree(home)
    task(request(f"/databases/{db}", "DELETE", {"confirm_name": PREFIX, "delete_data": True}, 202))
    run(["docker", "volume", "inspect", details["volume"]], success=False)
    print("PASS: non-root native consumer, stopped connect/disconnect, loopback PostgreSQL, restricted SQL role, credential revocation and fixture cleanup")


if __name__ == "__main__":
    try:
        main()
    except BaseException:
        print("FAIL: retained synthetic fixtures with prefix", PREFIX, flush=True)
        raise
