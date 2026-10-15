import assert from "node:assert/strict";
import { test } from "node:test";
import { addressesOf, canGrant, consumersOf, grantable, policyChange, providersOf } from "../src/lib/privateAccess.ts";

const provider = {
  id: "db000000000a", name: "database", compose: "services:\n  db:\n    image: postgres:17\n",
  publication: { kind: "unpublished" }, runtime: { kind: "container" },
  network_policy: { kind: "private", consumers: ["api00000000a"] },
  services: [{ service: "db", container: "sf-app-db000000000a-db", state: "running" }],
};
const consumer = { id: "api00000000a", name: "api", image: "nginx", publication: { kind: "web" }, runtime: { kind: "container" } };
const native = { id: "nat00000000a", name: "jellyfin", runtime: { kind: "native" } };
// Grants and is granted at the same time.
const both = {
  id: "mid00000000a", name: "cache", compose: "services:\n  redis:\n    image: redis\n",
  publication: { kind: "unpublished" }, runtime: { kind: "container" },
  network_policy: { kind: "private", consumers: [] },
  services: [{ service: "redis", container: "sf-app-mid00000000a-redis", state: "running" }],
};
const apps = [provider, consumer, native, { ...both, network_policy: { kind: "private", consumers: [] } }];

test("only an unpublished Compose Application in a container grants access", () => {
  assert.equal(canGrant(provider), true);
  assert.equal(canGrant(consumer), false, "a published Application has a Hostname instead");
  assert.equal(canGrant({ ...provider, runtime: { kind: "native" } }), false);
  assert.equal(canGrant({ id: "g", name: "g", publication: { kind: "unpublished" }, git: { compose_path: "compose.yaml" } }), true,
    "a Git Compose Application counts as Compose");
});

test("a shared or missing policy grants nobody", () => {
  assert.deepEqual(consumersOf({ ...provider, network_policy: { kind: "shared" } }), []);
  assert.deepEqual(consumersOf({ ...provider, network_policy: undefined }), []);
  assert.deepEqual(consumersOf(provider), ["api00000000a"]);
});

test("any other container Application can be granted; native ones and itself cannot", () => {
  assert.deepEqual(grantable(provider, apps).map((one) => one.name), ["api", "cache"]);
});

test("a consumer sees the providers that granted it, and their service addresses", () => {
  assert.deepEqual(providersOf(consumer, apps).map((one) => one.name), ["database"]);
  assert.deepEqual(providersOf(provider, apps), []);
  assert.deepEqual(addressesOf(provider), [{ service: "db", address: "sf-app-db000000000a-db" }]);
  assert.deepEqual(addressesOf({ ...provider, services: undefined }), [], "not run yet, so no container name to show");
});

test("an Application can grant and be granted at the same time", () => {
  const granted = [{ ...provider, network_policy: { kind: "private", consumers: ["mid00000000a"] } }, both];
  assert.deepEqual(providersOf(both, granted).map((one) => one.name), ["database"]);
  assert.equal(canGrant(both), true);
});

test("an edit sends the policy only when the grants changed", () => {
  assert.equal(policyChange(provider, ["api00000000a"]), undefined);
  assert.deepEqual(policyChange(provider, []), { kind: "private", consumers: [] });
  assert.deepEqual(policyChange(provider, ["api00000000a", "mid00000000a"]), { kind: "private", consumers: ["api00000000a", "mid00000000a"] });
  assert.deepEqual(policyChange({ ...provider, network_policy: { kind: "shared" } }, ["api00000000a"]),
    { kind: "private", consumers: ["api00000000a"] }, "the first grant turns a shared policy private");
});
