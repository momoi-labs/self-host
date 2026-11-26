import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { build } from "vite";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const outDir = await mkdtemp(join(root, ".failure-test-"));
after(() => rm(outDir, { recursive: true, force: true }));
await build({
  root, configFile: false, logLevel: "silent", esbuild: { jsx: "automatic" },
  build: {
    ssr: join(root, "src/components/Failure.tsx"), outDir,
    rollupOptions: { output: { entryFileNames: "Failure.mjs" } },
  },
});
const { Failure } = await import(pathToFileURL(join(outDir, "Failure.mjs")));

const report = { error: "The image did not start", caused_by: ["Docker refused the container", "port 8080 is in use"] };

test("an incident renders the loud alert with collapsed causes", () => {
  const html = renderToStaticMarkup(createElement(Failure, {
    failure: report, incident: "Last deploy failed", help: "Fix the cause, then deploy again.", actionLabel: "Retry", onAction() {},
  }));
  assert.match(html, /class="alert alert-danger alert-loud"/);
  assert.match(html, /data-tape="top"/);
  assert.match(html, /data-animated="true"/);
  assert.doesNotMatch(html, /data-pulse=/);
  assert.equal(html.match(/<svg/g)?.length, 2, "the loud mark and the details chevron only");
  assert.match(html, /class="alert-label">Error · Last deploy failed</);
  assert.match(html, /class="alert-message">The image did not start</);
  assert.match(html, /class="alert-help"><p><b>help:<\/b> Fix the cause, then deploy again.<\/p><button[^>]*>Retry<\/button>/);
  assert.match(html, /<div data-slot="alert-details" id="[^"]+" hidden="" class="alert-details"><ol class="alert-causes"><li>Docker refused the container<\/li><li>port 8080 is in use<\/li><\/ol><\/div>/);
  assert.match(html, /aria-expanded="false"[^>]*>.*Show causes<\/button>/);
});

test("an incident without causes has no details toggle", () => {
  const html = renderToStaticMarkup(createElement(Failure, { failure: "Build failed", incident: "Build failed" }));
  assert.match(html, /alert-loud/);
  assert.doesNotMatch(html, /alert-details/);
});

test("an inline failure stays tinted", () => {
  const html = renderToStaticMarkup(createElement(Failure, { failure: report }));
  assert.match(html, /class="alert alert-danger"/);
  assert.doesNotMatch(html, /alert-loud|data-tape/);
  assert.match(html, /<ol class="alert-causes">/);
});
