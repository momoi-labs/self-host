import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { build } from "vite";

// Render the real form and Kiso controls. Browser interactions and backend
// deployment remain real-console checks, not claims made by these tests.
const root = dirname(dirname(fileURLToPath(import.meta.url)));
const outDir = await mkdtemp(join(root, ".native-form-test-"));
after(() => rm(outDir, { recursive: true, force: true }));
await build({
  root, configFile: false, logLevel: "silent", esbuild: { jsx: "automatic" },
  build: {
    ssr: join(root, "src/components/AppForm.tsx"), outDir,
    rollupOptions: { output: { entryFileNames: "AppForm.mjs" } },
  },
});
const { AppForm } = await import(pathToFileURL(join(outDir, "AppForm.mjs")));

const app = {
  id: "native-app", name: "Native app", image: "", hostname: "native.home.lan", status: "running",
  runtime: { kind: "native", account: "sf_native", command: ["node", "server.js"], port: 8787 },
  publication: { kind: "web" },
};
const git = {
  repository: "https://github.com/team/worker.git", git_ref: "main", context: "packages/api",
  dockerfile: "Dockerfile", build_args: {}, build_secrets: {}, credential_id: "git-private",
};

function render(record) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(["native", "capabilities"], { available: true, resource_limits: true, metrics: true, terminal: true, reason: null });
  client.setQueryData(["source", "connections"], []);
  client.setQueryData(["tool-catalog"], []);
  client.setQueryData(["apps", record.id, "variable-names"], []);
  try {
    return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(AppForm, {
      app: record, dnsSuffix: "home.lan", onSubmit: () => assert.fail("render must not submit"),
    })));
  } finally {
    client.clear();
  }
}

function noContainerFields(html) {
  assert.doesNotMatch(html, /id="f-git-(dockerfile|compose|args|secrets|strict)"|id="f-registry-credential"|Pull newer images/);
}

test("a native Git record opens the native form with repository-only settings and a separate build", () => {
  const html = render({ ...app, git, runtime: { ...app.runtime, build: ["npm ci", "npm run build"] } });
  assert.match(html, /id="native-name"/);
  assert.match(html, /Repository URL/);
  assert.match(html, /Connection \(optional\)/);
  assert.match(html, /Branch or tag/);
  assert.match(html, /Pinned commit/);
  assert.match(html, /id="native-git-context"[^>]*value="packages\/api"/);
  assert.match(html, /id="native-build"/);
  assert.match(html, /npm ci\nnpm run build/);
  assert.match(html, /Save and build/);
  noContainerFields(html);
});

test("a standalone Worker shows its editable config and tools instead of an empty start command", () => {
  const html = render({
    ...app, git,
    runtime: {
      ...app.runtime, command: [], worker: { config: "workers/api.jsonc" },
      recipe: { dependencies: [
        { tool: "node", version: "24.22.2" },
        { tool: "npm:wrangler", version: "4.157.0" },
        { tool: "github:cloudflare/workerd", version: "1.20261103.1" },
        { tool: "python", version: "3.14" },
      ], setup: [] },
    },
  });
  const config = html.match(/<input[^>]*id="native-worker-config"[^>]*>/)?.[0] ?? "";
  assert.match(config, /required=""/);
  assert.match(config, /value="workers\/api.jsonc"/);
  assert.doesNotMatch(html, /id="native-command"/);
  for (const version of ["24.22.2", "4.157.0", "1.20261103.1", "3.14"]) assert.ok(html.includes(version));
  assert.match(html, /A module Worker \(main in the config\) or a static site \(assets in the config\)/);
  assert.match(html, /No Cloudflare storage bindings/);
  assert.match(html, /Port on 127\.0\.0\.1/);
  const port = html.match(/<input[^>]*id="native-port"[^>]*>/)?.[0] ?? "";
  assert.match(port, /required=""/);
  assert.match(port, /value="8787"/);
  noContainerFields(html);
});

test("older account-home native records retain their command and have no Git build or Worker config", () => {
  const html = render(app);
  assert.match(html, /Application home/);
  assert.match(html, /id="native-command"/);
  assert.match(html, /node server\.js/);
  assert.match(html, /id="native-setup"/);
  assert.doesNotMatch(html, /id="native-build"|id="native-worker-config"|Repository URL/);
  noContainerFields(html);
});
