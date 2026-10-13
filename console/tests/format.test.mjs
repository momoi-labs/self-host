import assert from "node:assert/strict";
import { test } from "node:test";
import { formatAgo } from "../src/lib/format.ts";

test("a commit's age reads in its coarsest unit", () => {
  const now = Date.parse("2026-10-13T12:00:00Z");
  assert.equal(formatAgo("2026-10-13T12:00:00Z", now), "just now");
  assert.equal(formatAgo("2026-10-13T11:51:00Z", now), "9 minutes ago");
  assert.equal(formatAgo("2026-10-13T11:00:00Z", now), "1 hour ago");
  assert.equal(formatAgo("2026-10-12T09:00:00+00:00", now), "1 day ago");
  assert.equal(formatAgo("2024-10-13T12:00:00Z", now), "2 years ago");
});
