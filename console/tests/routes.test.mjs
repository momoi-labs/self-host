import assert from "node:assert/strict";
import { after, test } from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "vite";

// routes.ts imports the API client, so it is built the way the console is.
const root = dirname(dirname(fileURLToPath(import.meta.url)));
const outDir = await mkdtemp(join(root, ".routes-test-"));
after(() => rm(outDir, { recursive: true, force: true }));
await build({
  root,
  configFile: false,
  logLevel: "silent",
  build: {
    ssr: join(root, "src/lib/routes.ts"),
    outDir,
    rollupOptions: { output: { entryFileNames: "routes.mjs" } },
  },
});
const { replaceRoute, routesOf } = await import(pathToFileURL(join(outDir, "routes.mjs")));

const app = {
  id: "web",
  name: "web",
  hostname: "web.home.lan",
  aliases: ["www.home.lan", "old.home.lan"],
  route_rules: [
    { hostname: "web.home.lan", path_prefix: "/api", strip_prefix: true },
    { hostname: "web.home.lan", path_prefix: "/metrics", target: "127.0.0.1:9100", strip_prefix: false },
  ],
};
const [, www, , api, metrics] = routesOf(app);

test("an Application's routes are its Hostname, then its aliases, then its paths", () => {
  assert.deepEqual(routesOf(app).map((route) => `${route.kind} ${route.hostname}${route.path}`), [
    "hostname web.home.lan/",
    "alias www.home.lan/",
    "alias old.home.lan/",
    "path web.home.lan/api",
    "path web.home.lan/metrics",
  ]);
  assert.deepEqual(routesOf({ ...app, publication: { kind: "unpublished" } }), []);
});

test("a changed alias keeps its place", () => {
  const next = replaceRoute(app, www, { hostname: "shop.home.lan", path: "/", stripPrefix: false });
  assert.deepEqual(next.aliases, ["shop.home.lan", "old.home.lan"]);
  assert.deepEqual(next.route_rules, app.route_rules);
});

test("an alias given a path becomes a rule that follows the Web Target", () => {
  const next = replaceRoute(app, www, { hostname: "www.home.lan", path: "/shop", stripPrefix: true });
  assert.deepEqual(next.aliases, ["old.home.lan"]);
  assert.deepEqual(next.route_rules.at(-1), { hostname: "www.home.lan", path_prefix: "/shop", strip_prefix: true });
});

test("a changed rule keeps the target it named, and a removed one goes", () => {
  const changed = replaceRoute(app, metrics, { hostname: "web.home.lan", path: "/stats", stripPrefix: true });
  assert.deepEqual(changed.route_rules[1], { hostname: "web.home.lan", path_prefix: "/stats", strip_prefix: true, target: "127.0.0.1:9100" });
  assert.deepEqual(replaceRoute(app, api, null).route_rules, [app.route_rules[1]]);
  assert.deepEqual(replaceRoute(app, api, null).aliases, app.aliases);
});
