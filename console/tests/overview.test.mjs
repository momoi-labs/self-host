import assert from "node:assert/strict";
import { test } from "node:test";

import {
  countOf, errorRate, failedCounts, needsAttention, perMinute, tallies, topWorkloads,
} from "../src/lib/overview.ts";

const app = (id, more = {}) => ({ id, name: id, status: "running", ...more });
const machine = (id, more = {}) => ({ id, config: { name: id }, state: "running", operation: null, ...more });
const sample = (cpu_percent, memory_bytes) => ({ cpu_percent, memory_bytes });

const apps = [
  app("web"),
  app("blog", { status: "failed", last_error: { error: "failed to deploy the Application", caused_by: [] } }),
  app("maindb", { managed_postgres: { major: 17 }, status: "failed", last_error: "Docker daemon is not running." }),
  app("nas", { status: "failed", runtime: { kind: "external" } }),
];
const machines = [
  machine("dev"),
  machine("lab", { operation: { action: "create", status: "failed", error: { error: "no disk space", caused_by: [] } } }),
];

test("needs attention lists failed apps, then databases, then machines, with the real error", () => {
  assert.deepEqual(needsAttention(apps, machines), [
    { id: "blog", name: "blog", kind: "application", summary: "failed to deploy the Application" },
    { id: "maindb", name: "maindb", kind: "database", summary: "Docker daemon is not running." },
    { id: "lab", name: "lab", kind: "machine", summary: "no disk space" },
  ]);
  assert.deepEqual(needsAttention([app("web")], [machine("dev")]), []);
});

test("an External app is not a workload", () => {
  assert.deepEqual(failedCounts(apps, machines), { application: 1, database: 1, machine: 1 });
  assert.deepEqual(tallies(apps, machines), {
    application: { running: 1, total: 2 },
    database: { running: 0, total: 1 },
    machine: { running: 2, total: 2 },
    all: { running: 3, total: 5 },
  });
});

test("a failed workload with no recorded error has no summary", () => {
  assert.equal(needsAttention([app("x", { status: "failed" })], [])[0].summary, null);
});

test("top workloads rank by CPU, then memory, and keep unmeasured ones last", () => {
  const metrics = {
    applications: [
      { id: "web", samples: [sample(1, 10), sample(5, 300)] },
      { id: "maindb", samples: [sample(5, 900)] },
      { id: "blog", samples: [] },
    ],
    machines: [{ id: "dev", samples: [sample(40, 1)] }],
  };
  const top = topWorkloads(apps, machines, metrics);
  assert.deepEqual(top.map((row) => [row.name, row.kind, row.cpu, row.memory]), [
    ["dev", "machine", 40, 1],
    ["maindb", "database", 5, 900],
    ["web", "application", 5, 300],
    ["blog", "application", null, null],
    ["lab", "machine", null, null],
  ]);
  assert.equal(top[3].samples, null);
  assert.equal(topWorkloads(apps, machines, metrics, 2).length, 2);
  assert.deepEqual(topWorkloads(apps, machines, null).map((row) => row.cpu), [null, null, null, null, null]);
});

test("rates read per minute and the error rate needs requests", () => {
  assert.equal(perMinute(5, 10), 30);
  assert.equal(perMinute(3, 0), 180);
  assert.equal(errorRate([]), null);
  assert.equal(errorRate([{ proxy_requests: 0, proxy_errors: 0 }]), null);
  assert.equal(errorRate([{ proxy_requests: 150, proxy_errors: 3 }, { proxy_requests: 50, proxy_errors: 1 }]), 2);
});

test("a count names one thing or many", () => {
  assert.equal(countOf(1, "app"), "1 app");
  assert.equal(countOf(0, "app"), "0 apps");
  assert.equal(countOf(2, "entry", "entries"), "2 entries");
});
