import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";

const [command, expectedVersion] = process.argv.slice(2);
const { version } = JSON.parse(readFileSync("package.json", "utf8"));
assert.match(version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/, "Invalid release version");

const manifests = [
  ["Cargo.toml", /(\[package\][\s\S]*?\nversion = ")([^"]+)(")/],
  ["Cargo.lock", /(\[\[package\]\]\nname = "self-host"\nversion = ")([^"]+)(")/],
];

if (command === "sync" || command === "check") {
  if (expectedVersion) assert.equal(version, expectedVersion, "Tag and package version differ");
  // Read and validate both files before changing either one.
  const files = manifests.map(([path, pattern]) => {
    const contents = readFileSync(path, "utf8");
    const match = contents.match(pattern);
    assert.ok(match, `Cannot find the self-host version in ${path}`);
    if (command === "check") assert.equal(match[2], version, `${path} version differs`);
    return [path, contents.replace(pattern, `$1${version}$3`)];
  });
  if (command === "sync") {
    for (const [path, contents] of files) writeFileSync(path, contents);
  }
} else if (command === "notes") {
  const changelog = readFileSync("CHANGELOG.md", "utf8");
  const heading = `## ${version}\n`;
  const start = changelog.indexOf(heading);
  assert.ok(start >= 0, `Missing changelog entry for ${version}`);
  const rest = changelog.slice(start + heading.length);
  const end = rest.indexOf("\n## ");
  const notes = (end < 0 ? rest : rest.slice(0, end)).trim();
  assert.ok(notes, `Empty changelog entry for ${version}`);
  console.log(notes);
} else {
  throw new Error("Usage: release-version.mjs sync | check [version] | notes");
}
