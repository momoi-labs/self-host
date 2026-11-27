import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "vite";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const outDir = await mkdtemp(join(root, ".use-view-test-"));
after(() => rm(outDir, { recursive: true, force: true }));
await build({
  root,
  configFile: false,
  logLevel: "silent",
  build: {
    ssr: join(root, "src/lib/useView.ts"),
    outDir,
    rollupOptions: { output: { entryFileNames: "useView.mjs" } },
  },
});
const { hashFor, viewFromHash } = await import(pathToFileURL(join(outDir, "useView.mjs")));

test("the workload lists have their own hashes", () => {
  assert.deepEqual(viewFromHash("#apps"), { view: "apps", id: null });
  assert.deepEqual(viewFromHash("#machines"), { view: "machines", id: null });
  assert.equal(hashFor({ view: "apps", id: null }), "#apps");
  assert.equal(hashFor({ view: "machines", id: null }), "#machines");
});

test("every hash from before the lists still opens its screen", () => {
  const known = {
    "#new": { view: "new", id: null },
    "#databases": { view: "databases", id: null },
    "#routes": { view: "routes", id: null },
    "#dns": { view: "dns", id: null },
    "#events": { view: "events", id: null },
    "#custom-images": { view: "custom-images", id: null },
    "#new-virtual-machine": { view: "environment-new", id: null },
    "#app-abc123": { view: "app", id: "abc123" },
    "#virtual-machine-vm1": { view: "environments", id: "vm1" },
  };
  for (const [hash, view] of Object.entries(known)) {
    assert.deepEqual(viewFromHash(hash), view, hash);
    assert.equal(hashFor(view), hash, JSON.stringify(view));
  }
});

test("the Overview has no hash", () => {
  assert.equal(viewFromHash(""), null);
  assert.equal(hashFor({ view: "overview", id: null }), undefined);
});

test("a hash is a list only when it is the whole hash", () => {
  assert.deepEqual(viewFromHash("#app-s1"), { view: "app", id: "s1" });
  assert.notEqual(viewFromHash("#apps").view, "app");
});
