import assert from "node:assert/strict";
import { test } from "node:test";
import { taskOutcome, waitForTask } from "../src/lib/tasks.ts";

test("a task has an outcome only once it is completed or failed", () => {
  const events = [
    { id: "queued", status: "pending" },
    { id: "started", status: "running" },
    { id: "done", status: "completed" },
    { id: "broken", status: "failed", error: { error: "no such image", caused_by: [] } },
  ];
  assert.equal(taskOutcome(events, "queued"), null);
  assert.equal(taskOutcome(events, "started"), null);
  assert.equal(taskOutcome(events, "missing"), null);
  assert.equal(taskOutcome(events, "done")?.status, "completed");
  assert.equal(taskOutcome(events, "broken")?.error.error, "no such image");
});

test("waiting on a task polls until the same id finishes", async () => {
  const polls = [
    [{ id: "task-1", status: "pending" }],
    [{ id: "task-1", status: "running" }],
    [{ id: "task-1", status: "failed", error: { error: "stopped", caused_by: [] } }],
  ];
  const delays = [];
  const outcome = await waitForTask("task-1", {
    events: async () => polls.shift() ?? [],
    delay: async (ms) => { delays.push(ms); },
    interval: 7,
  });
  assert.equal(outcome.status, "failed");
  assert.deepEqual(delays, [7, 7]);
  await assert.rejects(
    waitForTask("task-2", { events: async () => [], delay: async () => {}, attempts: 3 }),
    /Events page/,
  );
});
