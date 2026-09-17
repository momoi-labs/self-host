import assert from "node:assert/strict";
import { test } from "node:test";
import { recordRows } from "../src/lib/dnsRecords.ts";

test("groups published addresses and preserves ownership, origin, editability and application links", () => {
  const record = { type: "A", ttl: 60 };
  const rows = recordRows([
    { ...record, name: "*", value: "192.168.1.2", owner: "platform" },
    { ...record, name: "*", value: "10.0.0.2", owner: "platform" },
    { ...record, name: "admin", value: "192.168.1.2", owner: "platform" },
    { ...record, name: "photos", value: "192.168.1.2", owner: "application", application_id: "app-1" },
    { ...record, name: "pictures", value: "192.168.1.2", owner: "application", application_id: "app-1" },
    { ...record, name: "nas", value: "192.168.1.30", owner: "operator" },
  ], [{ id: "app-1", name: "Photo library" }]);
  assert.deepEqual(rows.map(({ name, owner, origin, editable, renamable, application_id, values }) =>
    ({ name, owner, origin, editable, renamable, application_id, values })), [
    { name: "*", owner: "platform", origin: "Wildcard", editable: false, renamable: false, application_id: undefined, values: ["192.168.1.2", "10.0.0.2"] },
    { name: "admin", owner: "platform", origin: "admin", editable: true, renamable: false, application_id: undefined, values: ["192.168.1.2"] },
    { name: "photos", owner: "application", origin: "Photo library", editable: false, renamable: false, application_id: "app-1", values: ["192.168.1.2"] },
    { name: "pictures", owner: "application", origin: "Photo library", editable: false, renamable: false, application_id: "app-1", values: ["192.168.1.2"] },
    { name: "nas", owner: "operator", origin: "Operator", editable: true, renamable: true, application_id: undefined, values: ["192.168.1.30"] },
  ]);
});

test("API validation errors reach the inline report without losing their cause", async () => {
  const { failureOf } = await import("../src/lib/api.ts");
  for (const [status, error] of [
    [409, "'storage' already has an A Record"],
    [409, "'photos' is owned by Application 'Photos' (app-1); edit the Application instead"],
    [400, "invalid Record: 'home.lan' is the apex of the Zone"],
    [400, "invalid Record: 'wrong' is not an IPv4 address"],
  ]) {
    const report = await failureOf(new Response(JSON.stringify({ error, caused_by: ["Details"] }), { status }));
    assert.deepEqual(report, { error, caused_by: ["Details"] });
  }
  assert.deepEqual(await failureOf(new Response("Unavailable", { status: 503 })),
    { error: "Unavailable", caused_by: [] });
});

test("an Application using admin still identifies its owning resource", () => {
  const [row] = recordRows([
    { type: "A", ttl: 60, value: "192.168.1.30", name: "admin", owner: "application", application_id: "app-1" },
  ], [{ id: "app-1", name: "Dashboard" }]);
  assert.equal(row.origin, "Dashboard");
  assert.equal(row.application_id, "app-1");
  assert.equal(row.editable, false);
});

test("a machine record links to its machine by ID and cannot be edited as an Operator record", () => {
  const [row] = recordRows([
    { name: "dev", type: "A", value: "192.168.1.41", ttl: 60, owner: "virtual-machine", virtual_machine_id: "vm-1" },
  ], [], [{ id: "vm-1", config: { name: "Workspace" } }]);
  assert.equal(row.origin, "Workspace");
  assert.equal(row.originHref, "/console/#environment-vm-1");
  assert.equal(row.owner, "virtual-machine");
  assert.equal(row.editable, false);
});
