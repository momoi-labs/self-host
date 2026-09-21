#!/usr/bin/env python3
"""Move Platform State written as JSON files into platform.db.

Platforms before ADR-0027 kept their state in state/platform.json and
state/applications/<id>/application.json. This one keeps it in
state/platform.db and refuses to start while platform.json is still there.

Run it with the daemon stopped, after the new version has started once and
created platform.db:

    python3 scripts/import-json-state.py ~/.config/self-host/state

Standard library only. The JSON files are moved to state/imported-<date>/,
not deleted; that folder is the rollback.
"""

from __future__ import annotations

import fcntl
import json
import sqlite3
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

RETENTION_DAYS = 30
COLLECTIONS = {
    "dns_records_v1": ("dns-record", lambda item: f"{item['name']}/{item['type']}"),
    "environments_v1": ("virtual-machine", lambda item: item["id"]),
    "custom_images_v1": ("custom-image", lambda item: item["id"]),
    "tasks_v1": ("task", lambda item: item["id"]),
}


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def is_terminal(status: str) -> bool:
    return status in ("completed", "failed")


def migrate_legacy_audit(events: list[dict]) -> list[dict]:
    """Old background work had separate acceptance and completion records.
    Merge only unambiguous pairs; a rejected second request is not a worker
    result. An acceptance left on its own becomes `pending`: it is history,
    and the scheduler never picks it up. This is what the daemon used to do at
    read time. Events are camelCase on disk, the way the API serves them."""
    events = sorted(events, key=lambda event: event["occurredAt"])
    result: list[dict] = []
    open_indexes: list[int] = []
    prefixes = ("Virtual machine operation ", "Application deployment ", "Custom image build ")
    for event in events:
        if event.get("updatedAt"):
            result.append(event)
            continue
        completion = is_terminal(event["status"]) and event["description"].startswith(prefixes)
        if completion:
            matches = [
                index
                for index in open_indexes
                if result[index]["action"] == event["action"]
                and result[index]["subject"]["kind"] == event["subject"]["kind"]
                and result[index]["subject"]["id"]
                and result[index]["subject"]["id"] == event["subject"]["id"]
                and result[index].get("apiName") == event.get("apiName")
            ]
            if len(matches) == 1:
                start = result[matches[0]]
                start["status"] = event["status"]
                start["description"] = event["description"]
                start["startedAt"] = start["occurredAt"]
                start["finishedAt"] = event["occurredAt"]
                start["updatedAt"] = event["occurredAt"]
                open_indexes.remove(matches[0])
                continue
        event["updatedAt"] = event["occurredAt"]
        if event["status"] == "accepted":
            event["status"] = "pending"
            open_indexes.append(len(result))
        elif completion:
            event["finishedAt"] = event["occurredAt"]
        result.append(event)
    return result


def main(argv: list[str]) -> None:
    if len(argv) != 2:
        fail("usage: import-json-state.py <state directory>")
    state = Path(argv[1]).expanduser()
    legacy = state / "platform.json"
    db_path = state / "platform.db"
    if not legacy.exists():
        fail(f"{legacy} does not exist; nothing to import")
    if not db_path.exists():
        fail(f"{db_path} does not exist; start the new self-host once so it creates the database")

    lock_path = state / "platform.lock"
    lock = open(lock_path, "a+")
    try:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        fail("the daemon is running; stop it before importing")

    platform = json.loads(legacy.read_text())
    if platform.get("version", 1) > 1:
        fail(f"{legacy} declares format {platform['version']}, which this script does not read")

    applications: list[tuple[Path, dict]] = []
    for record_path in sorted((state / "applications").glob("*/application.json")):
        applications.append((record_path, json.loads(record_path.read_text())))

    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA journal_mode = DELETE")
    conn.execute("PRAGMA synchronous = FULL")
    version = conn.execute("SELECT version FROM schema_version").fetchone()[0]
    if version != 1:
        fail(f"{db_path} is at schema version {version}; this script writes version 1")
    if conn.execute("SELECT count(*) FROM settings").fetchone()[0]:
        fail(f"{db_path} already holds settings; the import already ran")

    now = now_iso()
    counts: dict[str, int] = {}
    with conn:
        # Every scalar the daemon kept in the key-value map: api_key,
        # dns_suffix, host_ip, host_addresses, api_key_digest:<id>.
        for key, value in platform.get("state", {}).items():
            if key in COLLECTIONS or key == "audit_events_v1":
                continue
            conn.execute("INSERT INTO settings (key, value) VALUES (?, ?)", (key, value))
            counts["settings"] = counts.get("settings", 0) + 1
        for row in platform.get("api_keys", []):
            conn.execute(
                "INSERT INTO api_keys (id, label, created_at) VALUES (?, ?, ?)",
                (row["id"], row["label"], row["created_at"]),
            )
            counts["api_keys"] = counts.get("api_keys", 0) + 1
        for key, (kind, key_of) in COLLECTIONS.items():
            raw = platform.get("state", {}).get(key)
            if not raw:
                continue
            for item in json.loads(raw):
                conn.execute(
                    "INSERT INTO records (kind, id, body, updated_at) VALUES (?, ?, ?, ?)",
                    (kind, key_of(item), json.dumps(item, separators=(",", ":")), now),
                )
                counts[kind] = counts.get(kind, 0) + 1
        for _, record in applications:
            record.pop("version", None)
            conn.execute(
                "INSERT INTO records (kind, id, body, updated_at) VALUES (?, ?, ?, ?)",
                ("application", record["id"], json.dumps(record, separators=(",", ":")), now),
            )
            counts["application"] = counts.get("application", 0) + 1
        raw_audit = platform.get("state", {}).get("audit_events_v1")
        if raw_audit:
            cutoff = (datetime.now(timezone.utc) - timedelta(days=RETENTION_DAYS)).strftime(
                "%Y-%m-%dT%H:%M:%S.%fZ"
            )
            for event in migrate_legacy_audit(json.loads(raw_audit)):
                updated_at = event.get("updatedAt") or event["occurredAt"]
                if updated_at < cutoff:
                    counts["audit_pruned"] = counts.get("audit_pruned", 0) + 1
                    continue
                conn.execute(
                    "INSERT INTO audit_events"
                    " (id, status, subject_kind, subject_id, occurred_at, updated_at, body)"
                    " VALUES (?, ?, ?, ?, ?, ?, ?)",
                    (
                        event["id"],
                        event["status"],
                        event["subject"]["kind"],
                        event["subject"]["id"],
                        event["occurredAt"],
                        updated_at,
                        json.dumps(event, separators=(",", ":")),
                    ),
                )
                counts["audit_events"] = counts.get("audit_events", 0) + 1
    conn.close()

    imported = state / f"imported-{datetime.now(timezone.utc).strftime('%Y%m%d-%H%M%S')}"
    imported.mkdir(mode=0o700)
    legacy.rename(imported / legacy.name)
    for record_path, _ in applications:
        target = imported / "applications" / record_path.parent.name
        target.mkdir(parents=True, mode=0o700)
        record_path.rename(target / record_path.name)

    for name, count in sorted(counts.items()):
        print(f"{name}: {count}")
    print(f"JSON files moved to {imported}. Delete that folder once the console shows what you expect.")


if __name__ == "__main__":
    main(sys.argv)
