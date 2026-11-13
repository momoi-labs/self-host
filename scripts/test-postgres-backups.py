#!/usr/bin/env python3
"""Exercise PostgreSQL backups with point-in-time recovery (ADR-0035) against a disposable Host.

Never use a production daemon: this run replaces the Host's backup storage.
It refuses to start when the Host already has one.

SELF_HOST_TEST_URL points to the isolated API. SELF_HOST_TEST_KEY holds its key.
SELF_HOST_TEST_ANY_ADDRESS=1 allows an API that is not on loopback.
SELF_HOST_TEST_S3_IMAGE names the S3 server image (default: versity/versitygw).
SELF_HOST_TEST_S3_ADDRESS is an address of this Host that the daemon and the
database containers both reach (default: the address of the default route).
A firewall that blocks containers from the Host's ports needs an address it
lets through.

The run starts its own S3 server in Docker behind a TLS proxy with a
self-signed certificate. It makes one database server, turns its backups on,
writes a row, notes the time, writes a second row, verifies the latest backup,
and restores to the noted time into a new server. The new server must hold the
first row and not the second. It removes what it made. A failed run keeps its
fixtures for diagnosis and prints their names.
"""

import json
import os
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

BASE = os.environ["SELF_HOST_TEST_URL"].rstrip("/")
KEY = os.environ["SELF_HOST_TEST_KEY"]
MAJOR = 17
if not os.environ.get("SELF_HOST_TEST_ANY_ADDRESS"):
    assert urllib.parse.urlsplit(BASE).hostname in {"127.0.0.1", "localhost", "::1"}, "Use the disposable Host's loopback API"
PREFIX = "bk-test-" + uuid.uuid4().hex[:8]
S3_IMAGE = os.environ.get("SELF_HOST_TEST_S3_IMAGE", "versity/versitygw:latest")
NETWORK = PREFIX + "-net"
S3_CONTAINER = PREFIX + "-s3"
TLS_CONTAINER = PREFIX + "-tls"
ACCESS_KEY = "sfbackuptest"
SECRET_KEY = "sf-" + uuid.uuid4().hex
BUCKET = "sf-backups"


def request(path, method="GET", body=None, expected=200):
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Authorization": "Bearer " + KEY}
    if data is not None:
        headers["Content-Type"] = "application/json"
    try:
        response = urllib.request.urlopen(urllib.request.Request(BASE + path, data=data, method=method, headers=headers), timeout=120)
    except urllib.error.HTTPError as error:
        response = error
    raw = response.read()
    assert response.status == expected, f"{method} {path}: expected {expected}, got {response.status}: {raw[:300]!r}"
    return json.loads(raw) if raw else None


def task(accepted):
    for _ in range(1200):
        event = next((e for e in request("/events") if e["id"] == accepted["task_id"]), None)
        if event and event["status"] in {"completed", "failed"}:
            assert event["status"] == "completed", f"The Task failed: {event.get('error')}"
            return event
        time.sleep(1)
    raise AssertionError("Task did not finish within twenty minutes")


def docker(*args, data=None, env=None):
    result = subprocess.run(["docker", *args], input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=300, env=env)
    assert result.returncode == 0, f"docker {args[0]} failed: {result.stderr[-300:]!r}"
    return result.stdout


def psql(server, database, sql):
    return docker("exec", "--user", "postgres", "-i", f"sf-app-{server}-database", "psql", "--no-psqlrc", "--quiet", "--tuples-only", "--no-align", "--set", "ON_ERROR_STOP=1", "--username", "sf_admin", "--dbname", database, data=sql.encode()).decode().strip()


def host_address():
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.connect(("192.0.2.1", 9))
        return probe.getsockname()[0]


assert not request("/backups/storage")["configured"], "This Host already has a backup storage. Use a disposable Host"
print("Disposable fixture prefix:", PREFIX)

# An S3 server of this run's own. pgBackRest speaks HTTPS only, so a TLS
# proxy with a self-signed certificate stands in front of it.
address = os.environ.get("SELF_HOST_TEST_S3_ADDRESS") or host_address()
with socket.socket() as probe:
    probe.bind((address, 0))
    port = probe.getsockname()[1]
docker("network", "create", NETWORK)
docker("run", "--detach", "--name", S3_CONTAINER, "--network", NETWORK, "--tmpfs", "/data", "--env", "ROOT_ACCESS_KEY=" + ACCESS_KEY, "--env", "ROOT_SECRET_KEY", S3_IMAGE, "posix", "/data", env={**os.environ, "ROOT_SECRET_KEY": SECRET_KEY})
certificates = tempfile.mkdtemp(prefix=PREFIX)
os.chmod(certificates, 0o755)
subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=" + address, "-keyout", certificates + "/key.pem", "-out", certificates + "/cert.pem"], check=True, capture_output=True)
with open(certificates + "/both.pem", "w") as both:
    both.write(open(certificates + "/cert.pem").read() + open(certificates + "/key.pem").read())
os.chmod(certificates + "/both.pem", 0o644)
docker("run", "--detach", "--name", TLS_CONTAINER, "--network", NETWORK, "--publish", f"{address}:{port}:9943", "--volume", certificates + ":/tls:ro", "alpine/socat", "OPENSSL-LISTEN:9943,fork,reuseaddr,cert=/tls/both.pem,verify=0", f"TCP:{S3_CONTAINER}:7070")
time.sleep(3)
docker("exec", S3_CONTAINER, "mkdir", "-p", f"/data/{BUCKET}")

storage = {"endpoint": f"https://{address}:{port}", "region": "us-east-1", "bucket": BUCKET, "prefix": PREFIX, "path_style": True, "verify_tls": False, "access_key_id": ACCESS_KEY}

# Plain HTTP, a certificate nobody signed and a wrong secret are each refused, and nothing is saved.
request("/backups/storage/test", "POST", {**storage, "endpoint": f"http://{address}:{port}", "secret_access_key": SECRET_KEY}, 400)
request("/backups/storage/test", "POST", {**storage, "verify_tls": True, "secret_access_key": SECRET_KEY}, 422)
request("/backups/storage/test", "POST", {**storage, "secret_access_key": "wrong-" + SECRET_KEY}, 422)
request("/backups/storage/test", "POST", {**storage, "secret_access_key": SECRET_KEY})
assert not request("/backups/storage")["configured"]
saved = request("/backups/storage", "PUT", {**storage, "secret_access_key": SECRET_KEY})
assert saved["configured"] and saved["has_secret"] and saved["verify_tls"] is False
assert SECRET_KEY not in json.dumps(request("/backups/storage")), "The API returned the secret access key"
# Saving again without the secret keeps it.
request("/backups/storage", "PUT", storage)
request("/backups/storage/test", "POST")

created = request("/databases", "POST", {"name": PREFIX, "major": MAJOR}, 202)
task(created)
db = created["id"]

# One consumer database, made the way an Operator makes one with no
# Application: an import into a new name.
with tempfile.TemporaryDirectory() as folder:
    os.chmod(folder, 0o755)
    dump = os.path.join(folder, "blog.sql")
    with open(dump, "w") as out:
        out.write("CREATE TABLE notes (id integer PRIMARY KEY, body text NOT NULL);\nINSERT INTO notes SELECT n, 'note ' || n FROM generate_series(1, 500) n;\n")
    os.chmod(dump, 0o644)
    task(request(f"/databases/{db}/databases/blog/import-path", "POST", {"path": dump, "format": "sql"}, 202))

# Backups off: nothing to back up, verify or restore.
assert request(f"/databases/{db}/backups/settings")["enabled"] is False
request(f"/databases/{db}/backups", "POST", expected=409)
request(f"/databases/{db}/backups/settings", "PUT", {"keep_days": 0}, 400)

# Turning backups on changes the image, creates the repository and takes the first full backup.
turned_on = request(f"/databases/{db}/backups/settings", "PUT", {"enabled": True, "keep_days": 7})
task(turned_on)
settings = request(f"/databases/{db}/backups/settings")
assert settings["enabled"] and settings["keep_days"] == 7 and settings["next_run_at"] and settings["next_kind"] in {"full", "diff", "incr"}
assert settings["problem"] is None, settings["problem"]
backups = request(f"/databases/{db}/backups")
assert [b["kind"] for b in backups] == ["full"] and backups[0]["bytes"] > 0 and backups[0]["repository_bytes"] > 0
assert psql(db, "postgres", "SHOW archive_mode;") == "on"
assert psql(db, "blog", "SELECT count(*) FROM notes;") == "500", "Turning backups on lost data"
print("Backups on, first full backup:", backups[0]["label"])

# A row before the target time and a row after it.
psql(db, "blog", "INSERT INTO notes VALUES (1001, 'before the target');")
time.sleep(2)
target = psql(db, "postgres", "SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"');")
time.sleep(2)
psql(db, "blog", "INSERT INTO notes VALUES (1002, 'after the target');")

task(request(f"/databases/{db}/backups", "POST", expected=202))
backups = request(f"/databases/{db}/backups")
assert [b["kind"] for b in backups] == ["incr", "full"], "The newest backup comes first"
row = next(r for r in request("/backups") if r["database_application_id"] == db)
assert row["enabled"] and row["last"]["label"] == backups[0]["label"] and row["problem"] is None

# Verify restores the latest backup into a throwaway container and removes it.
task(request(f"/databases/{db}/backups/verify", "POST", expected=202))
verification = request(f"/databases/{db}/backups/settings")["verification"]
assert verification["status"] == "passed" and verification["label"] == backups[0]["label"]
assert not docker("ps", "--all", "--quiet", "--filter", "name=sf-backup-verify-").strip(), "Verify left its container behind"
print("Verification passed")

# What cannot be restored is refused before any Task starts.
request(f"/databases/{db}/backups/restore", "POST", {"name": PREFIX}, 409)
request(f"/databases/{db}/backups/restore", "POST", {"name": PREFIX + "-x", "time": "2999-01-01T00:00:00Z"}, 400)
request(f"/databases/{db}/backups/restore", "POST", {"name": PREFIX + "-x", "time": "yesterday"}, 400)
request("/backups/storage", "DELETE", expected=409)

# Point-in-time recovery: a new server at the target time.
copy_name = PREFIX + "-pitr"
restored = request(f"/databases/{db}/backups/restore", "POST", {"name": copy_name, "time": target}, 202)
task(restored)
copy = restored["id"]
assert copy != db
assert psql(copy, "blog", "SELECT string_agg(id::text, ',' ORDER BY id) FROM notes WHERE id > 1000;") == "1001", "The new server is not at the target time"
assert psql(copy, "blog", "SELECT count(*) FROM notes;") == "501"
assert psql(copy, "postgres", "SELECT pg_is_in_recovery();") == "f"
assert psql(copy, "postgres", "SHOW archive_mode;") == "off", "The restored server would write into the source's repository"
assert request(f"/databases/{copy}/backups/settings")["enabled"] is False
assert psql(db, "blog", "SELECT count(*) FROM notes;") == "502", "The restore touched the source server"
print("Restored to", target, "as", copy_name)

events = json.dumps(request("/events"))
assert SECRET_KEY not in events, "An event carries the secret access key"

# Turning backups off puts the official image back and keeps the data.
task(request(f"/databases/{db}/backups/settings", "PUT", {"enabled": False}))
assert request(f"/databases/{db}/backups/settings")["enabled"] is False
assert psql(db, "postgres", "SHOW archive_mode;") == "off"
assert psql(db, "blog", "SELECT count(*) FROM notes;") == "502"

for name, server in [(copy_name, copy), (PREFIX, db)]:
    task(request(f"/databases/{server}", "DELETE", {"confirm_name": name, "delete_data": True}, 202))
request("/backups/storage", "DELETE", expected=204)
docker("rm", "--force", "--volumes", TLS_CONTAINER, S3_CONTAINER)
docker("network", "rm", NETWORK)
print("PostgreSQL backups: enable, back up, verify, restore to a time and disable all passed")
