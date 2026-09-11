import assert from "node:assert/strict";
import { test } from "node:test";
import {
  isKey, isVersion, miseToml, readOption, splitKey, suggest, takesAllowBuilds,
} from "../src/lib/dependencies.ts";

const catalog = [
  { name: "node", backends: ["core:node"] },
  { name: "just", backends: ["asdf:just", "aqua:just"] },
  { name: "t3", backends: ["npm:t3"] },
];

test("offers an exact key first, then a prefix, then a match inside", () => {
  assert.deepEqual(suggest(catalog, "just", []), ["just", "aqua:just", "asdf:just"]);
});

test("keeps entry working when the catalog answered nothing", () => {
  assert.deepEqual(suggest([], "just", []), ["just"]);
  assert.deepEqual(suggest([], "npm:t3", []), ["npm:t3"]);
  assert.deepEqual(suggest([], "not a key", []), []);
});

test("leads with a backend key even when the catalog names the tool", () => {
  assert.equal(suggest(catalog, "npm:t3", [])[0], "npm:t3");
});

test("drops what the recipe already holds", () => {
  assert.deepEqual(suggest(catalog, "just", ["just"]), ["aqua:just", "asdf:just"]);
});

test("splits a tool the way a chip reads it", () => {
  assert.deepEqual(splitKey("npm:t3"), { scope: "npm", name: "t3" });
  assert.deepEqual(splitKey("node"), { name: "node" });
});

test("accepts the versions mise takes and refuses the rest", () => {
  for (const version of ["latest", "24", "1.2.3", "v1.2.3-rc.1"]) {
    assert.equal(isVersion(version), true, version);
  }
  for (const version of ["", " ", "^1.2", "1.2 3", "-1"]) {
    assert.equal(isVersion(version), false, JSON.stringify(version));
  }
});

test("reads an option back as the chip prints it", () => {
  assert.deepEqual(readOption("allow_builds=[node-pty, esbuild]"), {
    name: "allow_builds", values: ["node-pty", "esbuild"],
  });
  assert.deepEqual(readOption("allow_builds=node-pty"), {
    name: "allow_builds", values: ["node-pty"],
  });
  assert.deepEqual(readOption("allow_builds="), { name: "allow_builds", values: [] });
  assert.equal(readOption("node-pty"), null);
});

test("offers allow_builds only where mise reads it", () => {
  assert.equal(takesAllowBuilds("npm:t3"), true);
  assert.equal(takesAllowBuilds("node"), false);
});

test("writes a tool with options as a table and the rest as a version", () => {
  assert.equal(
    miseToml(
      [{ tool: "node", version: "24" }, { tool: "npm:t3", version: "latest", allow_builds: ["node-pty"] }],
      ["t3 --help"],
    ),
    '[tools]\n"node" = "24"\n"npm:t3" = { version = "latest", allow_builds = ["node-pty"] }\n'
      + '\n[tasks.check]\nrun = ["t3 --help"]\n',
  );
});

test("accepts a key the console would send to mise", () => {
  assert.equal(isKey("npm:@openai/codex"), true);
  assert.equal(isKey("claude-code"), true);
  assert.equal(isKey(""), false);
  assert.equal(isKey("two words"), false);
});
