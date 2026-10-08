import assert from "node:assert/strict";
import { test } from "node:test";
import {
  isKey, isOptionName, isVersion, readOption, setOption, splitKey, suggest, takesAllowBuilds,
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

test("writes any mise option a chip names, and moves one renamed", () => {
  const pypi = { tool: "pypi:laya-apple", version: "1.6.3" };
  const extras = setOption(pypi, "extras=serve, ane");
  assert.deepEqual(extras, { ...pypi, allow_builds: undefined, options: { extras: ["serve", "ane"] } });
  assert.deepEqual(setOption(extras, "with=[pip]", "extras").options, { with: ["pip"] });
  assert.equal(setOption(extras, "extras=", "extras").options, undefined);
  assert.equal(setOption(pypi, "Extras=serve"), null);
  assert.equal(setOption(pypi, "version=2"), null);
  assert.equal(setOption(pypi, "allow_builds=node-pty"), null);
});

test("keeps allow_builds in its own field on npm tools", () => {
  const npm = { tool: "npm:t3", version: "latest", allow_builds: ["node-pty"] };
  assert.deepEqual(setOption(npm, "os=macos", "allow_builds"), {
    ...npm, allow_builds: undefined, options: { os: ["macos"] },
  });
  assert.deepEqual(setOption(npm, "allow_builds=").allow_builds, undefined);
  assert.equal(isOptionName("default-features"), true);
  assert.equal(isOptionName("allow_builds"), false);
});

test("accepts a key the console would send to mise", () => {
  assert.equal(isKey("npm:@openai/codex"), true);
  assert.equal(isKey("claude-code"), true);
  assert.equal(isKey(""), false);
  assert.equal(isKey("two words"), false);
});
