import assert from "node:assert/strict";
import { test } from "node:test";
import { barSteps, parseRun, phase, stepViews } from "../src/lib/runSteps.ts";

const failedUpdate = `2026-09-18T17:00:00Z bootstrap --- bootstrap started ---
2026-09-18T17:00:00Z bootstrap starting
2026-09-18T17:00:06Z bootstrap provisioning
2026-09-18T17:00:07Z bootstrap SF_STEP system-packages
2026-09-18T17:00:07Z bootstrap Get:1 http://archive.ubuntu.com noble InRelease
2026-09-18T17:00:55Z bootstrap SF_STEP user-and-ssh
2026-09-18T17:00:57Z bootstrap SF_STEP mise
2026-09-18T17:01:06Z bootstrap SF_STEP tools
2026-09-18T17:01:06Z bootstrap mise install node@24 ... done (cached)
2026-09-18T17:02:58Z bootstrap mise ERROR no backend found for "opencode"
2026-09-18T17:02:58Z bootstrap SF_STEP failed:tools
2026-09-18T17:02:59Z bootstrap --- bootstrap failed: Provisioning failed at tools. ---
`;

test("reads the failed step, its output and the reason", () => {
  const run = parseRun(failedUpdate);
  assert.equal(run.action, "bootstrap");
  assert.equal(run.status, "failed");
  assert.equal(run.failedStep, "tools");
  assert.equal(run.message, "Provisioning failed at tools.");
  assert.equal(run.startedAt, "2026-09-18T17:00:00Z");
  assert.equal(run.endedAt, "2026-09-18T17:02:59Z");
  const tools = run.steps.find((step) => step.name === "tools");
  assert.deepEqual(tools.output, [
    "mise install node@24 ... done (cached)",
    'mise ERROR no backend found for "opencode"',
  ]);
});

test("lays the run over the steps the action walks", () => {
  const views = stepViews(parseRun(failedUpdate));
  assert.deepEqual(
    views.map((view) => [view.name, view.state, view.seconds]),
    [
      ["starting", "done", 6],
      ["provisioning", "done", 1],
      ["system-packages", "done", 48],
      ["user-and-ssh", "done", 2],
      ["mise", "done", 9],
      ["tools", "failed", 112],
      ["setup", "pending", null],
      ["checks", "pending", null],
      ["service", "pending", null],
      ["health", "pending", null],
      ["ready", "pending", null],
    ],
  );
});

test("only the last run counts", () => {
  const log =
    failedUpdate +
    `2026-09-18T17:10:00Z start --- start started ---
2026-09-18T17:10:00Z start starting
2026-09-18T17:10:04Z start --- start succeeded ---
`;
  const run = parseRun(log);
  assert.equal(run.action, "start");
  assert.equal(run.status, "succeeded");
  // The Host's start never prints "ready"; a run that succeeded walked it.
  assert.deepEqual(
    stepViews(run).map((view) => [view.name, view.state]),
    [
      ["starting", "done"],
      ["ready", "done"],
    ],
  );
});

test("a stage the run walked past was not needed", () => {
  // An update of a machine that is already up never starts it.
  const run = parseRun(`2026-09-18T17:40:00Z update --- update started ---
2026-09-18T17:40:00Z update provisioning
2026-09-18T17:40:03Z update SF_STEP system-packages
`);
  const views = stepViews(run);
  assert.equal(views.find((view) => view.name === "starting").state, "skipped");
  assert.equal(views.find((view) => view.name === "provisioning").state, "done");
  assert.equal(views.find((view) => view.name === "system-packages").state, "running");
  assert.equal(views.find((view) => view.name === "user-and-ssh").state, "pending");
});

test("a run with no closing line is still going", () => {
  const run = parseRun(failedUpdate.split("\n").slice(0, 8).join("\n"));
  assert.equal(run.status, "running");
  const views = stepViews(run);
  assert.equal(views.find((view) => view.name === "tools").state, "running");
  assert.equal(views.find((view) => view.name === "mise").state, "done");
});

test("a guest line that is one word does not open a step", () => {
  const run = parseRun(`2026-09-18T17:00:00Z bootstrap --- bootstrap started ---
2026-09-18T17:00:00Z bootstrap starting
2026-09-18T17:00:01Z bootstrap ready
2026-09-18T17:00:02Z bootstrap Reading package lists...
2026-09-18T17:00:03Z bootstrap done
`);
  assert.deepEqual(run.steps.map((step) => step.name), ["starting", "ready"]);
  assert.deepEqual(run.steps[1].output, ["Reading package lists...", "done"]);
});

test("nothing recorded is no run", () => {
  assert.equal(parseRun(""), null);
  assert.equal(parseRun("garbage\n"), null);
});

test("a bar drawn from the step the record names", () => {
  assert.deepEqual(
    barSteps("start", "starting").map((step) => step.state),
    ["running", "pending"],
  );
  assert.deepEqual(
    barSteps("create", "mise").map((step) => step.state).slice(0, 6),
    ["done", "done", "done", "done", "running", "pending"],
  );
  assert.ok(barSteps("create", null).every((step) => step.state === "pending"));
  assert.deepEqual(
    barSteps("create", "mise", true).map((step) => step.state).slice(3, 6),
    ["done", "failed", "pending"],
  );
});

test("a step names the phase the run is in", () => {
  assert.equal(phase("create", "creating"), "Creating");
  assert.equal(phase("create", "booting"), "Starting");
  assert.equal(phase("create", "tools"), "Provisioning");
  assert.equal(phase("create", "ready"), "Ready");
  assert.equal(phase("bootstrap", null), "Provisioning");
  assert.equal(phase("stop", "stopping"), "Stopping");
});
