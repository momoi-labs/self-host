import assert from "node:assert/strict";
import { test } from "node:test";
import { detectFormat } from "../src/lib/archiveFormat.ts";

const bytes = (text) => new TextEncoder().encode(text);

test("a dump's type is read from its first bytes", () => {
  assert.equal(detectFormat(bytes("PGDMP\u0001")), "custom");
  const tar = new Uint8Array(512);
  tar.set(bytes("ustar"), 257);
  assert.equal(detectFormat(tar), "tar");
  assert.equal(detectFormat(bytes("--\n-- PostgreSQL database dump\n")), "sql");
});

test("nothing is preselected when the type cannot be told", () => {
  assert.equal(detectFormat(new Uint8Array([0x1f, 0x8b, 0x08, 0x00])), null, "compressed");
  assert.equal(detectFormat(bytes("hello")), null);
  assert.equal(detectFormat(new Uint8Array()), null);
});
