import assert from "node:assert/strict";
import { test } from "node:test";
import { logEventParser } from "../src/lib/logEvents.ts";

test("preserves QR whitespace across chunks and ignores SSE comments", () => {
  const parse = logEventParser();
  assert.deepEqual(parse("data:   ▄  █  \r"), []);
  assert.deepEqual(parse("\ndata: \r\n\r\n: keepalive\n\ndata: keepalive\n\n"), [
    { text: "  ▄  █  ", level: undefined },
    { text: "", level: undefined },
    { text: "keepalive", level: undefined },
  ]);
});

test("keeps a notice's level across data lines and resets at the next event", () => {
  const parse = logEventParser();
  assert.deepEqual(parse("event: notice\ndata:first\ndata: second\n\ndata: third\n\n"), [
    { text: "first", level: "info" }, { text: "second", level: "info" },
    { text: "third", level: undefined },
  ]);
});
