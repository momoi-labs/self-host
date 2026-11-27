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
const { Failure, splitCauses } = await import(pathToFileURL(join(outDir, "Failure.mjs")));

const chain = [
  "failed to create the Docker network",
  "failed to connect to the docker API",
  "dial unix /run/docker.sock",
  "connect",
  "no such file or directory",
];

const render = (props) => renderToStaticMarkup(createElement(Failure, props));

test("the root cause joins the trailing plain causes and the framing lines are steps", () => {
  assert.deepEqual(splitCauses(chain), {
    steps: ["failed to create the Docker network", "failed to connect to the docker API"],
    root: "dial unix /run/docker.sock: connect: no such file or directory",
  });
  assert.deepEqual(splitCauses(["port 8080 is in use"]), { steps: [], root: "port 8080 is in use" });
  assert.deepEqual(splitCauses([]), { steps: [], root: null });
});

test("a cause chain renders the loud alert with the root cause and a peek", () => {
  const html = render({ failure: { error: "failed to deploy", caused_by: chain }, incident: "Last deploy failed", actionLabel: "Retry", onAction() {} });
  assert.match(html, /class="alert alert-danger alert-loud"/);
  assert.match(html, /data-tape="top"/);
  assert.match(html, /data-animated="true"/);
  assert.doesNotMatch(html, /data-pulse=/);
  assert.doesNotMatch(html, /lucide-circle-alert/);
  assert.match(html, /class="alert-label">Error · Last deploy failed</);
  assert.match(html, /<div class="failure-story-actions"><button[^>]*class="[^"]*failure-story-toggle"[^>]*aria-expanded="false"[^>]*>.*?Details<\/button><button[^>]*class="[^"]*failure-story-primary"[^>]*>Retry<\/button><\/div>/);
  assert.match(html, /<li class="failure-story-root">.*?Root cause<\/span><p[^>]*>dial unix \/run\/docker.sock: connect: no such file or directory<\/p>/);
  assert.match(html, /<li class="failure-story-peek"[^>]*>.*?Triggered while<\/span><p>failed to connect to the docker API<\/p><\/div><\/li><\/ol>/);
  assert.doesNotMatch(html, /failure-story-result|failure-story-step/);
});

test("a failure that left the workload running keeps the graph in a tinted alert", () => {
  const html = render({ failure: { error: "failed to deploy", caused_by: chain }, incident: "Last deploy failed", blocking: false, actionLabel: "Retry", onAction() {} });
  assert.match(html, /class="alert alert-danger failure-story-quiet"/);
  assert.doesNotMatch(html, /alert-loud|data-tape|data-animated/);
  assert.match(html, /class="alert-label">Error · Last deploy failed</);
  assert.match(html, /<li class="failure-story-root">.*?Root cause<\/span><p[^>]*>dial unix \/run\/docker.sock: connect: no such file or directory<\/p>/);
  assert.match(html, /failure-story-primary/);
});

test("a single cause peeks at the result", () => {
  const html = render({ failure: { error: "failed to deploy", caused_by: ["port 8080 is in use"] }, incident: "Last deploy failed" });
  assert.match(html, /Root cause<\/span><p[^>]*>port 8080 is in use<\/p>/);
  assert.match(html, /<li class="failure-story-peek"[^>]*>.*?Result<\/span><p>Failed to deploy<\/p>/);
  assert.match(html, /failure-story-toggle/);
  assert.doesNotMatch(html, /failure-story-primary/);
});

test("a report with no causes is one flush cause with no graph or Details", () => {
  const html = render({ failure: "docker is down", incident: "Last operation failed" });
  assert.match(html, /<div class="failure-story-single"><span class="failure-story-kicker">Cause<\/span><p[^>]*>Docker is down<\/p><\/div>/);
  assert.doesNotMatch(html, /failure-story-rail|failure-story-lane|failure-story-actions|<circle class="failure-story/);
});

test("an inline failure stays tinted", () => {
  const html = render({ failure: { error: "The image did not start", caused_by: ["port 8080 is in use"] } });
  assert.match(html, /class="alert alert-danger"/);
  assert.doesNotMatch(html, /alert-loud|data-tape|failure-story/);
  assert.match(html, /<ol class="alert-causes"><li>port 8080 is in use<\/li><\/ol>/);
});
