import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const script = fileURLToPath(new URL("./release-cargo.sh", import.meta.url));
const targets = [
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-apple-darwin",
  "aarch64-apple-darwin",
];

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), "release-artifacts-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const prebuilt = join(cwd, "downloaded binaries");
  return {
    cwd,
    artifact(target, mode = 0o755) {
      const directory = join(prebuilt, target);
      mkdirSync(directory, { recursive: true });
      const file = join(directory, "self-host");
      writeFileSync(file, Buffer.from(`binary for ${target}\0\xff`, "latin1"));
      chmodSync(file, mode);
      return file;
    },
    run(target, directory = prebuilt) {
      return spawnSync("bash", [script, "build", `--target=${target}`, "--release", "--locked"], {
        cwd,
        encoding: "utf8",
        env: { ...process.env, SELF_HOST_PREBUILT_DIR: directory },
      });
    },
  };
}

test("imports all four targets without changing their bytes or executable mode", (t) => {
  const f = fixture(t);
  for (const target of targets) {
    const source = f.artifact(target);
    const result = f.run(target);
    assert.equal(result.status, 0, result.stderr);
    const output = join(f.cwd, "target", target, "release", "self-host");
    assert.deepEqual(readFileSync(output), readFileSync(source));
    assert.ok(statSync(output).mode & 0o111);
  }
});

test("rejects missing and non-executable artifacts even with an old build present", (t) => {
  const f = fixture(t);
  const target = targets[0];
  const oldBuild = join(f.cwd, "target", target, "release");
  mkdirSync(oldBuild, { recursive: true });
  writeFileSync(join(oldBuild, "self-host"), "stale binary");
  assert.notEqual(f.run(target).status, 0);
  f.artifact(target, 0o644);
  assert.notEqual(f.run(target).status, 0);
});

test("rejects unknown targets and an unset artifact directory", (t) => {
  const f = fixture(t);
  assert.notEqual(f.run("unknown-target").status, 0);
  assert.notEqual(f.run(targets[0], "").status, 0);
});
