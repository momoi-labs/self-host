import assert from "node:assert/strict";
import { test } from "node:test";
import { nativeCommand } from "../src/lib/nativeCommand.ts";
import { nativeRecipe } from "../src/lib/nativeRecipe.ts";
import { nativeGitBuild, nativeGitFields, nativeGitSource, workerDependencies } from "../src/lib/nativeGit.ts";

const gitFields = {
  repository: " https://github.com/team/app.git ",
  gitRef: " main ", revision: " abc123 ", context: " packages/api ",
  credentialId: " git-private ",
  // A native submission must not leak any container build settings.
  dockerfile: "Otherfile", composePath: "compose.yaml",
  args: "INVALID", secrets: "secret=private", registryCredentialId: "registry-private",
  username: "unused", token: "unused", strict: true,
};

test("native Git keeps repository access, ref, pin and context without container settings", () => {
  assert.deepEqual(nativeGitSource(gitFields), {
    repository: "https://github.com/team/app.git", git_ref: "main", revision: "abc123",
    context: "packages/api", credential_id: "git-private",
    dockerfile: "Dockerfile", build_args: {}, build_secrets: {},
  });
});

test("native Git defaults do not require new fields on older records", () => {
  assert.deepEqual(nativeGitFields(), { build: "", worker: false, workerConfig: "wrangler.jsonc" });
  assert.deepEqual(nativeGitFields(null), nativeGitFields());
  assert.deepEqual(nativeGitFields({ account: "app", command: ["node", "server.js"] }), nativeGitFields());
  const source = nativeGitSource({ ...gitFields, gitRef: " ", revision: " ", context: " ", credentialId: " " });
  assert.equal(source.git_ref, "HEAD");
  assert.equal(source.context, ".");
  assert.equal(source.revision, undefined);
  assert.equal(source.credential_id, undefined);
});

test("Git candidate build commands stay separate from Application Account setup", () => {
  const fields = {
    ...nativeGitFields(), build: "\r\n  npm ci  \r\nnpm run build\n\n",
    dependencies: [{ tool: "node", version: "24.22.2" }], setup: "mkdir -p state\n",
  };
  assert.deepEqual(nativeGitBuild(fields), { build: ["  npm ci  ", "npm run build"], worker: null });
  assert.deepEqual(nativeRecipe(fields), {
    dependencies: [{ tool: "node", version: "24.22.2" }], setup: ["mkdir -p state"],
  });
});

test("native Git build and Worker config round-trip independently of start command", () => {
  const native = {
    account: "app", command: [], build: ["npm ci", "npm run prepare"],
    worker: { config: "workers/api.jsonc" },
  };
  assert.deepEqual(nativeGitBuild(nativeGitFields(native)), {
    build: native.build, worker: native.worker,
  });
  assert.deepEqual(nativeCommand({ command: "", worker: true }), []);
  assert.deepEqual(nativeCommand({ command: "node server.js", worker: false }), ["node", "server.js"]);
  assert.throws(() => nativeCommand({ command: "", worker: false }), /Enter a start command/);
  assert.deepEqual(nativeGitBuild({ ...nativeGitFields(native), workerConfig: " workers/custom.toml " }).worker,
    { config: "workers/custom.toml" });
});

test("Worker selection installs the supported tools without dropping custom dependencies or options", () => {
  const existing = [
    { tool: "node", version: "22", options: { compile: ["true"] } },
    { tool: "python", version: "3.14" },
    { tool: "npm:custom-tool", version: "2", allow_builds: ["custom-build"] },
  ];
  const before = structuredClone(existing);
  const dependencies = workerDependencies(existing);
  assert.deepEqual(dependencies, [
    { tool: "node", version: "24.22.2", options: { compile: ["true"] } },
    existing[1], existing[2],
    { tool: "npm:wrangler", version: "4.157.0" },
    { tool: "github:cloudflare/workerd", version: "1.20261103.1" },
  ]);
  assert.deepEqual(existing, before);
  assert.deepEqual(workerDependencies(dependencies), dependencies);
  assert.deepEqual(workerDependencies([]).map(({ tool, version }) => `${tool}@${version}`), [
    "node@24.22.2", "npm:wrangler@4.157.0", "github:cloudflare/workerd@1.20261103.1",
  ]);
});
