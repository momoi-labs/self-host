import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { build } from "vite";

// Render the actual shell, including Kiso's navigation, without a browser.
const root = dirname(dirname(fileURLToPath(import.meta.url)));
const outDir = await mkdtemp(join(root, ".shell-test-"));
after(() => rm(outDir, { recursive: true, force: true }));
await build({
  root,
  configFile: false,
  logLevel: "silent",
  esbuild: { jsx: "automatic" },
  build: {
    ssr: join(root, "src/components/Shell.tsx"),
    outDir,
    rollupOptions: { output: { entryFileNames: "Shell.mjs" } },
  },
});
const { Shell } = await import(pathToFileURL(join(outDir, "Shell.mjs")));

function render(apps, environments, selected = "") {
  return renderToStaticMarkup(createElement(Shell, {
    crumb: "Overview", dnsSuffix: "home.lan", healthy: true, version: "0.4.0",
    apps, environments,
    overview: { href: "/console/", active: !selected },
    deploy: { href: "/console/#new", active: false },
    application: app => ({ href: `/console/#app-${app.id}`, active: selected === app.id }),
    virtualMachine: vm => ({ href: `/console/#environment-${vm.id}`, active: selected === vm.id }),
    children: null,
  }));
}
const app = { id: "photos", name: "Photos", status: "running" };
const vm = { id: "workspace", config: { name: "Workspace" }, state: "stopped" };

test("empty inventory has no resource headings or placeholder", () => {
  const html = render([], []);
  assert.doesNotMatch(html, /Applications|Virtual machines|None yet|No resources/);
});

test("the header shows the running Host version", () => {
  const html = render([], []);
  assert.match(html, /Healthy/);
  assert.match(html, /v0\.4\.0/);
});

test("VM-only inventory hides Applications and links to the VM", () => {
  const html = render([], [vm], vm.id);
  assert.doesNotMatch(html, /Applications/);
  assert.match(html, /Virtual machines/);
  assert.match(html, /href="\/console\/#environment-workspace"/);
  assert.match(html, /aria-current="page"[^>]*>.*Workspace/);
  assert.match(html, /data-variant="neutral"/);
});

test("apps-only inventory hides Virtual machines", () => {
  const html = render([app], []);
  assert.match(html, /Applications/);
  assert.match(html, /href="\/console\/#app-photos"/);
  assert.doesNotMatch(html, /Virtual machines/);
});

test("both types stay separate, with status dots and shortcuts first", () => {
  const html = render([{ ...app, status: "failed" }], [{ ...vm, state: "running" }]);
  assert.ok(html.indexOf("Custom images") < html.indexOf("Applications"));
  assert.ok(html.indexOf("Applications") < html.indexOf("Virtual machines"));
  assert.match(html, /data-variant="danger"/);
  assert.match(html, /href="\/console\/#dns"/);
  assert.match(html, /href="\/console\/#environment-workspace"/);
});
