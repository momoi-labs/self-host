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

function render(selected = "", dnsSuffix = "home.lan", failed = undefined) {
  return renderToStaticMarkup(createElement(Shell, {
    crumb: "Overview", dnsSuffix, healthy: true, degraded: false, version: "0.4.0", failed,
    overview: { href: "/console/", active: selected === "overview" },
    apps: { href: "/console/#apps", active: selected === "apps" },
    machines: { href: "/console/#machines", active: selected === "machines" },
    children: null,
  }));
}

test("the sidebar lists pages in groups, not single records", () => {
  const html = render();
  assert.doesNotMatch(html, /#app-|#virtual-machine-/);
  const order = ["Overview", "Workloads", "Apps", "Databases", "Machines", "Images", "Network", "Routes", "DNS", "System", "Events", "Settings"]
    .map((label) => html.indexOf(`>${label}<`));
  assert.ok(order.every((at) => at >= 0), `every label renders: ${order}`);
  assert.deepEqual(order, [...order].sort((a, b) => a - b));
  assert.match(html, /href="\/console\/#apps"/);
  assert.match(html, /href="\/console\/#machines"/);
  assert.match(html, /href="\/console\/#dns"/);
});

test("the current destination is marked", () => {
  assert.match(render("overview"), /aria-current="page"[^>]*>.*Overview/);
  assert.match(render("apps"), /href="\/console\/#apps"[^>]*aria-current="page"|aria-current="page"[^>]*href="\/console\/#apps"/);
});

test("a workload list shows its failed count, and nothing at zero", () => {
  assert.doesNotMatch(render(), /failed<\/span>/);
  const html = render("", "home.lan", { application: 2, database: 0, machine: 1 });
  assert.doesNotMatch(html, />0<span class="offscreen"> failed<\/span>/);
  assert.match(html, /Apps.*?>2<span class="offscreen"> failed<\/span>/);
  assert.match(html, /Machines.*?>1<span class="offscreen"> failed<\/span>/);
});

test("the breadcrumb root names the DNS suffix and falls back to Console", () => {
  assert.match(render(), /<a[^>]*href="\/console\/"[^>]*>home\.lan<\/a>/);
  const unset = render("", null);
  assert.match(unset, /<a[^>]*href="\/console\/"[^>]*>Console<\/a>/);
  assert.doesNotMatch(unset, /console-dns/);
});
