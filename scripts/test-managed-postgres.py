#!/usr/bin/env python3
"""Exercise P2 against a disposable Linux Host. Never use a production daemon.

SELF_HOST_TEST_URL points to the isolated API. SELF_HOST_TEST_KEY holds its key.
Only records/containers/volumes created by this run are removed. A failed run
retains its fixtures for diagnosis and prints their synthetic names.
"""

import json
import os
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE = os.environ["SELF_HOST_TEST_URL"].rstrip("/")
KEY = os.environ["SELF_HOST_TEST_KEY"]
MAJOR = 18
assert urllib.parse.urlsplit(BASE).hostname in {"127.0.0.1", "localhost", "::1"}, "Use the disposable Host's loopback API"
PREFIX = "pg-test-" + uuid.uuid4().hex[:8]


def request(path, method="GET", body=None, expected=200, authenticated=True):
    data = body if isinstance(body, bytes) else json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/octet-stream" if isinstance(body, bytes) else "application/json"}
    if authenticated:
        headers["Authorization"] = "Bearer " + KEY
    try:
        response = urllib.request.urlopen(urllib.request.Request(BASE + path, data=data, method=method, headers=headers), timeout=60)
    except urllib.error.HTTPError as error:
        response = error
    assert response.status == expected, f"{method} {path}: expected {expected}, got {response.status}"
    raw = response.read()
    return json.loads(raw) if raw else None


def task(accepted, failed=False):
    for _ in range(240):
        event = next((e for e in request("/events") if e["id"] == accepted["task_id"]), None)
        if event and event["status"] in {"completed", "failed"}:
            assert event["status"] == ("failed" if failed else "completed"), "Unexpected Task outcome"
            return
        time.sleep(1)
    raise AssertionError("Task did not finish within four minutes")


def connection(database, identifier, status):
    for _ in range(120):
        current = next(c for c in request(f"/databases/{database}")["connections"] if c["id"] == identifier)
        if current["status"] == status:
            return current
        assert current["status"] != "failed", "Connection failed"
        time.sleep(1)
    raise AssertionError("Connection did not settle")


def docker(*args, data=None, success=True):
    result = subprocess.run(["docker", *args], input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=180)
    assert (result.returncode == 0) == success, "Unexpected Docker or PostgreSQL command outcome"
    return result.stdout


def query(container, url, sql, success=True):
    parsed = urllib.parse.urlsplit(url)
    # The password enters stdin. It never appears in command arguments.
    script = 'IFS= read -r PGPASSWORD; export PGPASSWORD; exec psql --no-password --quiet --tuples-only --no-align --set ON_ERROR_STOP=1 --host "$1" --port "$2" --username "$3" --dbname "$4"'
    return docker("exec", "--user", "postgres", "-i", container, "sh", "-c", script, "query", parsed.hostname, str(parsed.port or 5432), parsed.username, parsed.path.lstrip("/"), data=(parsed.password + "\n" + sql).encode(), success=success).decode().strip()


def app(name):
    compose = f"services:\n  client:\n    image: postgres:{MAJOR}-bookworm\n    user: postgres\n    command: sleep infinity\n    environment:\n      DATABASE_URL: ${{DATABASE_URL:-}}\n"
    accepted = request("/apps", "POST", {"name": name, "compose": compose, "publication": {"kind": "unpublished"}}, 202)
    task(accepted)
    return accepted["id"], f"sf-app-{accepted['id']}-client"


def assert_stopped(identifier, container):
    assert request(f"/apps/id/{identifier}")["status"] == "stopped", "The consumer lost its stopped intent"
    assert docker("inspect", "--format", "{{.State.Running}}", container).strip() == b"false", "A stopped consumer was started while its Variables changed"


def assert_storage(container, volume):
    mounts = json.loads(docker("inspect", "--format", "{{json .Mounts}}", container))
    data_mounts = [mount for mount in mounts if mount["Destination"].startswith("/var/lib/postgresql")]
    assert len(data_mounts) == 1, "The database has an unexpected or anonymous data mount"
    mount = data_mounts[0]
    assert mount["Type"] == "volume" and mount["Name"] == volume and mount["RW"], "The database lost its persistent named volume"
    assert mount["Destination"] == "/var/lib/postgresql", "PostgreSQL 18 requires the versioned volume layout"
    settings = docker("exec", "--user", "postgres", "-i", container, "psql", "--quiet", "--tuples-only", "--no-align", "--username", "sf_admin", "--dbname", "postgres", data=b"SHOW data_directory;\nSHOW server_version_num;\n").decode().splitlines()
    assert settings[0] == "/var/lib/postgresql/18/docker", "PostgreSQL data is outside the expected versioned directory"
    assert int(settings[1]) // 10000 == MAJOR, "The database changed its selected major version"


print("Disposable fixture prefix:", PREFIX)
created = request("/databases", "POST", {"name": PREFIX, "major": MAJOR}, 202)
task(created)
db = created["id"]
provider = f"sf-app-{db}-database"
details = request(f"/databases/{db}")
assert details["readiness"] == "healthy" and details["major"] == MAJOR
assert_storage(provider, details["volume"])
record = request(f"/apps/id/{db}")
assert record["publication"] == {"kind": "unpublished"} and not record["hostname"]
assert not docker("port", provider).strip(), "A database without native consumers must publish no port"
request(f"/apps/id/{db}", "PUT", {"image": "postgres:16"}, 409)

first, client = app(PREFIX + "-client")
second, target_client = app(PREFIX + "-target")
task(request(f"/apps/id/{second}/stop", "POST", expected=202))
connections = []
urls = []
for consumer in [first, second]:
    accepted = request(f"/databases/{db}/connections", "POST", {"consumer_application_id": consumer, "variable": "DATABASE_URL"}, 202)
    task(accepted)
    identifier = accepted["connection_id"]
    connection(db, identifier, "ready")
    connections.append(identifier)
    request(f"/databases/{db}/connections/{identifier}/reveal", "POST", expected=401, authenticated=False)
    urls.append(request(f"/databases/{db}/connections/{identifier}/reveal", "POST")["url"])
assert_stopped(second, target_client)
assert dict(request(f"/apps/{PREFIX}-target/env"))["DATABASE_URL"] == urls[1]

metadata = json.dumps(request(f"/databases/{db}"))
for url in urls:
    assert urllib.parse.urlsplit(url).password not in metadata, "Metadata exposed a connection password"
request(f"/apps/{PREFIX}-client/env", "POST", {"key": "DATABASE_URL", "value": "replace"}, 409)
request(f"/databases/{db}", "DELETE", {"confirm_name": PREFIX, "delete_data": True}, 409)
query(client, urls[0], "CREATE TABLE evidence (value integer); INSERT INTO evidence VALUES (41);")
task(request(f"/apps/id/{db}/restart", "POST", {"pull": False}, 202))
assert_storage(provider, details["volume"])
assert query(client, urls[0], "SELECT value FROM evidence;") == "41"

source = urllib.parse.urlsplit(urls[0])
dump = docker("exec", "--user", "postgres", provider, "pg_dump", "--format=custom", "--username", source.username, "--dbname", source.path.lstrip("/"))
# A runtime started outside the API must not be mistaken for a stopped
# consumer merely because the persisted Application still says stopped.
docker("start", target_client)
task(request(f"/databases/{db}/connections/{connections[1]}/import", "POST", dump, 202), failed=True)
assert connection(db, connections[1], "ready")["imported_objects"] is None
task(request(f"/apps/id/{second}/stop", "POST", expected=202))
assert_stopped(second, target_client)
accepted = request(f"/databases/{db}/connections/{connections[1]}/import", "POST", dump, 202)
task(accepted)
assert connection(db, connections[1], "ready")["imported_objects"] == 1
task(request(f"/apps/id/{second}/start", "POST", expected=202))
assert query(target_client, urls[1], "SELECT value FROM evidence;") == "41"
# Both the original and imported rows must survive replacing the PG18 container.
previous_container = docker("inspect", "--format", "{{.Id}}", provider)
task(request(f"/databases/{db}/redeploy", "POST", expected=202))
assert docker("inspect", "--format", "{{.Id}}", provider) != previous_container, "Recreate did not replace the database container"
assert_storage(provider, details["volume"])
assert query(client, urls[0], "SELECT value FROM evidence;") == "41"
assert query(target_client, urls[1], "SELECT value FROM evidence;") == "41"
# Import refuses existing data and keeps the row.
task(request(f"/apps/id/{second}/stop", "POST", expected=202))
task(request(f"/databases/{db}/connections/{connections[1]}/import", "POST", dump, 202), failed=True)
task(request(f"/apps/id/{second}/start", "POST", expected=202))
assert query(target_client, urls[1], "SELECT value FROM evidence;") == "41"

for identifier in connections:
    if identifier == connections[1]:
        task(request(f"/apps/id/{second}/stop", "POST", expected=202))
    task(request(f"/databases/{db}/connections/{identifier}", "DELETE", expected=202))
    connection(db, identifier, "revoked")
assert_stopped(second, target_client)
query(client, urls[0], "SELECT 1;", success=False)
# A revoke accepted before connection work settles must never be overwritten.
pending = request(f"/databases/{db}/connections", "POST", {"consumer_application_id": first, "variable": "RACE_DATABASE_URL"}, 202)
revoking = request(f"/databases/{db}/connections/{pending['connection_id']}", "DELETE", expected=202)
task(pending)
task(revoking)
connection(db, pending["connection_id"], "revoked")
assert "RACE_DATABASE_URL" not in dict(request(f"/apps/{PREFIX}-client/env"))
for consumer_name in [PREFIX + "-client", PREFIX + "-target"]:
    values = dict(request(f"/apps/{consumer_name}/env"))
    assert "DATABASE_URL" not in values
    task(request(f"/apps/{consumer_name}", "DELETE", expected=202))
task(request(f"/databases/{db}", "DELETE", {"confirm_name": PREFIX, "delete_data": False}, 202))
docker("volume", "inspect", details["volume"])
# This is the synthetic volume this test explicitly created, never adopted data.
docker("volume", "rm", details["volume"])
disposable_name = PREFIX + "-delete"
disposable = request("/databases", "POST", {"name": disposable_name, "major": MAJOR}, 202)
task(disposable)
disposable_volume = request(f"/databases/{disposable['id']}")["volume"]
task(request(f"/databases/{disposable['id']}", "DELETE", {"confirm_name": disposable_name, "delete_data": True}, 202))
docker("volume", "inspect", disposable_volume, success=False)
print("PASS: PostgreSQL 18 versioned storage, readiness, private access, per-consumer credentials, stopped intent, guarded changes, restart/recreate persistence, observed-stop import checks, logical import, revocation and retained data")
