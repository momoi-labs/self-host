import assert from "node:assert/strict";
import { test } from "node:test";
import { fingerprintOf } from "../src/lib/fileFingerprint.ts";

const bytes = (text) => new TextEncoder().encode(text);

test("the same file has the same fingerprint, so its upload resumes", () => {
  assert.equal(fingerprintOf(10, 1, bytes("PGDMP head"), bytes("tail")), fingerprintOf(10, 1, bytes("PGDMP head"), bytes("tail")));
});

test("another file has another fingerprint, so its upload starts over", () => {
  const one = fingerprintOf(10, 1, bytes("PGDMP head"), bytes("tail"));
  assert.notEqual(one, fingerprintOf(10, 1, bytes("PGDMP head"), bytes("tall")), "other last bytes");
  assert.notEqual(one, fingerprintOf(10, 1, bytes("PGDMP haed"), bytes("tail")), "other first bytes");
  assert.notEqual(one, fingerprintOf(11, 1, bytes("PGDMP head"), bytes("tail")), "other size");
  assert.notEqual(one, fingerprintOf(10, 2, bytes("PGDMP head"), bytes("tail")), "changed since");
  assert.match(one, /^[A-Za-z0-9._:-]{1,128}$/, "the server takes it");
});
