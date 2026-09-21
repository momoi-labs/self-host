import assert from "node:assert/strict";
import { test } from "node:test";
import { eventActionLabel, eventSubjectHref, filterEvents, eventUpdatedAt, pageItems } from "../src/lib/platformEvents.ts";

test("resource links target existing console pages by stable ID", () => {
  assert.equal(eventSubjectHref({ kind: "application", id: "app-123", name: "renamed" }), "/console/#app-app-123");
  assert.equal(eventSubjectHref({ kind: "virtual-machine", id: "env-123", name: "vm" }), "/console/#environment-env-123");
  assert.equal(eventSubjectHref({ kind: "custom-image", id: "image-123", name: "image" }), "/console/#custom-image-image-123");
  assert.equal(eventSubjectHref({ kind: "application", id: "old-id", name: "removed", available: false }), null);
  assert.equal(eventSubjectHref({ kind: "application", id: "", name: "failed creation" }), null);
});

test("a DNS record event is labelled but has no screen to link to yet", () => {
  const subject = { kind: "dns-record", id: "nas/A", name: "nas", available: true };
  assert.equal(eventSubjectHref(subject), null);
  assert.equal(eventActionLabel({ action: "create", subject }), "Create DNS record");
  assert.equal(eventActionLabel({ action: "configure", subject }), "Configure DNS record");
});

test("filters by resource or service and status, then orders newest first", () => {
  const events = [
    { id: "older", occurredAt: "2026-09-13T11:00:00Z", status: "completed", subject: { name: "photos", service: "database" } },
    { id: "newer", occurredAt: "2026-09-13T12:00:00Z", status: "failed", subject: { name: "photos", service: "server" } },
  ];
  assert.deepEqual(filterEvents(events, " PHOTOS ", "all").map(event => event.id), ["newer", "older"]);
  assert.deepEqual(filterEvents(events, "database", "completed").map(event => event.id), ["older"]);
  assert.deepEqual(filterEvents(events, "database", "failed"), []);
  assert.deepEqual(events.map(event => event.id), ["older", "newer"]);
});

test("the table orders and displays the last update while keeping start and finish", () => {
  const finished = { id: "finished", occurredAt: "2026-09-14T08:00:00Z", startedAt: "2026-09-14T08:00:00Z", finishedAt: "2026-09-14T08:05:00Z", updatedAt: "2026-09-14T08:05:00Z", status: "completed", subject: { name: "vm" } };
  const running = { id: "running", occurredAt: "2026-09-14T08:03:00Z", status: "running", subject: { name: "other" } };
  assert.equal(eventUpdatedAt(finished), finished.finishedAt);
  assert.deepEqual(filterEvents([running, finished], "", "all").map(event => event.id), ["finished", "running"]);
  assert.equal(finished.startedAt, "2026-09-14T08:00:00Z");
});

test("the pager keeps the ends and the current page's neighbours, with one gap per skip", () => {
  assert.deepEqual(pageItems(1, 1), [1]);
  assert.deepEqual(pageItems(1, 4), [1, 2, "gap", 4]);
  assert.deepEqual(pageItems(5, 9), [1, "gap", 4, 5, 6, "gap", 9]);
  assert.deepEqual(pageItems(9, 9), [1, "gap", 8, 9]);
});
