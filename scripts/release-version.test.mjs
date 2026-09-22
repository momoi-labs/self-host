import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const script = resolve("scripts/release-version.mjs");
function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), "self-host-version-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  writeFileSync(join(cwd, "package.json"), JSON.stringify({ version: "0.4.0" }));
  writeFileSync(join(cwd, "Cargo.toml"), '[package]\nname = "self-host"\nversion = "0.3.0"\n\n[dependencies]\nother = "0.3.0"\n');
  writeFileSync(join(cwd, "Cargo.lock"), 'version = 4\n\n[[package]]\nname = "other"\nversion = "0.3.0"\n\n[[package]]\nname = "self-host"\nversion = "0.3.0"\n');
  return cwd;
}
function run(cwd, ...args) {
  return spawnSync(process.execPath, [script, ...args], { cwd, encoding: "utf8" });
}

test("sync changes only the crate version and rejects a mismatched tag", (t) => {
  const cwd = fixture(t);
  assert.notEqual(run(cwd, "check").status, 0);
  assert.equal(run(cwd, "sync").status, 0);
  assert.equal(run(cwd, "check", "0.4.0").status, 0);
  assert.notEqual(run(cwd, "check", "0.5.0").status, 0);
  assert.match(readFileSync(join(cwd, "Cargo.toml"), "utf8"), /other = "0.3.0"/);
  assert.match(readFileSync(join(cwd, "Cargo.lock"), "utf8"), /name = "other"\nversion = "0.3.0"/);
});

test("sync fails without partially writing malformed manifests", (t) => {
  const cwd = fixture(t);
  writeFileSync(join(cwd, "Cargo.lock"), "version = 4\n");
  assert.notEqual(run(cwd, "sync").status, 0);
  assert.match(readFileSync(join(cwd, "Cargo.toml"), "utf8"), /version = "0.3.0"/);
});

test("notes contains only the current release and requires an entry", (t) => {
  const cwd = fixture(t);
  writeFileSync(join(cwd, "CHANGELOG.md"), "# self-host\n\n## 0.4.0\n\n### Minor Changes\n\n- Packages.\n\n## 0.3.0\n\n- Previous release.\n");
  assert.equal(run(cwd, "notes").stdout, "### Minor Changes\n\n- Packages.\n");
  writeFileSync(join(cwd, "CHANGELOG.md"), "# self-host\n");
  assert.notEqual(run(cwd, "notes").status, 0);
});

test("Changesets versions the private Rust package and tags it only once", (t) => {
  const cwd = fixture(t);
  for (const path of ["package.json", "package-lock.json", ".changeset/config.json", "scripts/release-version.mjs"]) {
    cpSync(resolve(path), join(cwd, path), { recursive: true });
  }
  const pkg = JSON.parse(readFileSync(join(cwd, "package.json")));
  pkg.version = "0.3.0";
  writeFileSync(join(cwd, "package.json"), JSON.stringify(pkg));
  writeFileSync(join(cwd, ".changeset/test.md"), '---\n"self-host": minor\n---\n\nPublish packages.\n');
  symlinkSync(resolve("node_modules"), join(cwd, "node_modules"), "dir");
  const exec = (cmd, args, env = {}) => {
    const result = spawnSync(cmd, args, {
      cwd,
      encoding: "utf8",
      env: { ...process.env, npm_config_offline: "true", ...env },
    });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    return result.stdout;
  };
  exec("git", ["init", "-b", "main"]);
  exec("git", ["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "--allow-empty", "-m", "test"]);
  exec("git", ["tag", "v0.3.0"]);
  exec("npm", ["run", "version:packages"]);
  const version = JSON.parse(readFileSync(join(cwd, "package.json"))).version;
  assert.equal(version, "0.4.0");
  assert.equal(run(cwd, "check", version).status, 0);
  assert.equal(run(cwd, "notes").status, 0);
  const output = join(cwd, "events.ndjson");
  const env = { CHANGESETS_OUTPUT: output, GIT_COMMITTER_NAME: "Test", GIT_COMMITTER_EMAIL: "test@example.com" };
  exec("npm", ["run", "release:tag"], env);
  const event = JSON.parse(readFileSync(output, "utf8").trim());
  assert.deepEqual(event, { type: "git-tag", tag: "v0.4.0", packageName: "self-host" });
  assert.match(exec("git", ["tag", "--list"]), /v0.4.0/);
  writeFileSync(output, "");
  exec("npm", ["run", "release:tag"], env);
  assert.equal(readFileSync(output, "utf8"), "");
});
