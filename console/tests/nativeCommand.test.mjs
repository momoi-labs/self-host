import assert from "node:assert/strict";
import { test } from "node:test";
import { nativeCommand, nativeCommandFields } from "../src/lib/nativeCommand.ts";

test("one command field preserves multiline, quoted and empty arguments", () => {
  const command = ["/bin/sh", "-c", "printf '%s\\n' ready\nprintf '%s\\n' done\n", "", "  spaced  ", "a\r\nb", "maçã"];
  const fields = nativeCommandFields(command);
  assert.equal(typeof fields.command, "string");
  assert.deepEqual(nativeCommand(fields), command);
});

test("no arguments and one empty argument remain distinct", () => {
  assert.deepEqual(nativeCommand(nativeCommandFields(["/bin/true"])), ["/bin/true"]);
  assert.deepEqual(nativeCommand(nativeCommandFields(["/bin/true", ""])), ["/bin/true", ""]);
  assert.deepEqual(nativeCommandFields(), { command: "" });
});

test("typing a command keeps quoted spaces and literal variables", () => {
  assert.deepEqual(nativeCommand({ command: 'node server.js --title "two words" --value \'$HOME\' --empty \'\'' }),
    ["node", "server.js", "--title", "two words", "--value", "$HOME", "--empty", ""]);
});

test("malformed or empty commands fail before an API request", () => {
  for (const command of ['node "unclosed', "node trailing\\", "", "  ", "''"]) {
    assert.throws(() => nativeCommand({ command }), /start command/);
  }
});
