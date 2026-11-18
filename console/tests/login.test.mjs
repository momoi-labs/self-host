import assert from "node:assert/strict";
import { test } from "node:test";
import { checkApiKey } from "../src/lib/login.ts";

test("accepted keys are trimmed and checked with a bearer header", async () => {
  const result = await checkApiKey("  test-key\n", async (url, options) => {
    assert.equal(url, "/whoami");
    assert.deepEqual(options.headers, { Authorization: "Bearer test-key" });
    return Response.json({ kind: "human", label: "admin" });
  });
  assert.deepEqual(result, { kind: "accepted", key: "test-key" });
});

test("agent keys are refused without opening the console", async () => {
  const result = await checkApiKey("agent-key", async () => Response.json({ kind: "agent", label: "claude" }));
  assert.equal(result.kind, "key");
  assert.match(result.message, /agent key/);
  assert.ok(!result.message.includes("agent-key"));
});

test("empty or malformed keys never send a request", async () => {
  for (const key of ["", "  \n", "key with space", "key\nsecond", "non-ascii-é"]) {
    const result = await checkApiKey(key, () => assert.fail("must not send invalid credentials"));
    assert.equal(result.kind, "key");
  }
});

test("authentication refusals identify the key, service failures identify the Host", async () => {
  for (const status of [401, 403, 429, 500, 503]) {
    const result = await checkApiKey("test-key", async () => new Response(null, { status }));
    assert.equal(result.kind, status === 401 || status === 403 ? "key" : "host");
    assert.ok(!result.message.includes("test-key"));
  }
});

test("network errors have actionable copy without exposing browser error text", async () => {
  const result = await checkApiKey("test-key", async () => { throw new TypeError("Failed to fetch secret detail"); });
  assert.equal(result.kind, "host");
  assert.match(result.message, /LAN connection/);
  assert.ok(!result.message.includes("secret detail"));
});
