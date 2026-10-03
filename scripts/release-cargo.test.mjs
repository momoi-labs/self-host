import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
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
const s6Tools = ["s6-svscan", "s6-supervise", "s6-svscanctl", "s6-svc", "s6-svwait", "s6-svok", "s6-svstat", "s6-log", "s6-setlock", "s6-ftrigrd"];

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
      if (target.endsWith("-linux-gnu")) {
        const bundle = join(directory, "s6");
        mkdirSync(join(bundle, "bin"), { recursive: true });
        mkdirSync(join(bundle, "licenses"));
        for (const tool of s6Tools) {
          writeFileSync(join(bundle, "bin", tool), `binary ${tool} for ${target}`, { mode: 0o755 });
        }
        for (const license of ["s6", "skalibs", "musl", "zig"]) {
          writeFileSync(join(bundle, "licenses", `${license}.txt`), `${license} license`);
        }
        writeFileSync(join(bundle, "versions.txt"), `target ${target}\n`);
      }
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
    const s6 = join(f.cwd, "target", target, "release", "s6");
    if (target.endsWith("-linux-gnu")) {
      for (const tool of s6Tools) {
        assert.equal(readFileSync(join(s6, "bin", tool), "utf8"), `binary ${tool} for ${target}`);
        assert.ok(statSync(join(s6, "bin", tool)).mode & 0o111);
      }
      assert.equal(readFileSync(join(s6, "licenses", "s6.txt"), "utf8"), "s6 license");
    } else {
      assert.equal(existsSync(s6), false);
    }
  }
});

test("rejects an incomplete Linux bundle even when a previous complete build exists", (t) => {
  const f = fixture(t);
  const target = targets[0];
  f.artifact(target);
  assert.equal(f.run(target).status, 0);
  const bundle = join(f.cwd, "downloaded binaries", target, "s6");
  for (const tool of s6Tools) {
    const path = join(bundle, "bin", tool);
    chmodSync(path, 0o644);
    assert.notEqual(f.run(target).status, 0, `accepted non-executable ${tool}`);
    chmodSync(path, 0o755);
  }
  rmSync(join(bundle, "bin", "s6-ftrigrd"));
  assert.notEqual(f.run(target).status, 0);
});

test("rejects missing license notices or a bundle for the wrong architecture", (t) => {
  const f = fixture(t);
  const target = targets[0];
  f.artifact(target);
  const bundle = join(f.cwd, "downloaded binaries", target, "s6");
  writeFileSync(join(bundle, "versions.txt"), `target ${targets[1]}\n`);
  assert.notEqual(f.run(target).status, 0);
  writeFileSync(join(bundle, "versions.txt"), `target ${target}\n`);
  for (const license of ["s6", "skalibs", "musl", "zig"]) {
    const path = join(bundle, "licenses", `${license}.txt`);
    rmSync(path);
    assert.notEqual(f.run(target).status, 0);
    writeFileSync(path, `${license} license`);
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
