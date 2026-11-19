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

function render(selected = "", dnsSuffix = "home.lan", degraded = false) {
  return renderToStaticMarkup(createElement(Shell, {
    crumb: "Overview", dnsSuffix, healthy: true, degraded, version: "0.4.0",
    overview: { href: "/console/", active: selected === "overview" },
    children: null,
  }));
}

test("the sidebar lists pages, not applications or virtual machines", () => {
  const html = render();
  assert.doesNotMatch(html, /Applications|Virtual machines|#app-|#virtual-machine-/);
  assert.ok(html.indexOf("Custom images") < html.indexOf("Settings"));
  assert.match(html, /href="\/console\/#dns"/);
});

test("the current destination is marked", () => {
  assert.match(render("overview"), /aria-current="page"[^>]*>.*Overview/);
});

test("a degraded Host marks Overview in the navigation", () => {
  assert.doesNotMatch(render(), /Overview<\/span><span[^>]*class="dot/);
  assert.match(render("", "home.lan", true), /Overview<\/span>.*class="dot warning dot-pulse".*Degraded/);
});

test("the breadcrumb root names the DNS suffix and falls back to Console", () => {
  assert.match(render(), /<a[^>]*href="\/console\/"[^>]*>home\.lan<\/a>/);
  const unset = render("", null);
  assert.match(unset, /<a[^>]*href="\/console\/"[^>]*>Console<\/a>/);
  assert.doesNotMatch(unset, /console-dns/);
});
