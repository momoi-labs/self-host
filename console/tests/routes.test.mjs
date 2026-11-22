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
const { nameFor, replaceRoute, routesOf, sameFingerprint, targetLabel } = await import(pathToFileURL(join(outDir, "routes.mjs")));

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

test("a redirect draft reads back from the rule it writes, and lands on the route that holds its path", async () => {
  const { redirectRule, redirectDraft, redirectFrom, redirectTo, landingRoute } = await import(pathToFileURL(join(outDir, "routes.mjs")));
  const root = { hostname: "www.home.lan", path: "/", location: "/admin", permanent: false };
  const rule = redirectRule(root);
  assert.deepEqual(rule, { pattern: "^https?://www\\.home\\.lan/?$", replacement: "https://www.home.lan/admin", permanent: false });
  assert.deepEqual(redirectDraft(rule), root);
  assert.equal(redirectFrom(rule), "www.home.lan/");
  assert.equal(redirectTo(rule), "/admin");

  const moved = { hostname: "web.home.lan", path: "/old.html", location: "https://elsewhere.example.com/new", permanent: true };
  assert.equal(redirectRule(moved).pattern, "^https?://web\\.home\\.lan/old\\.html/?$");
  assert.deepEqual(redirectDraft(redirectRule(moved)), moved);

  assert.equal(redirectDraft({ pattern: "^https://old\\.home\\.lan/(.*)", replacement: "https://www.home.lan/${1}", permanent: true }), null);
  assert.equal(landingRoute(routesOf(app), rule.replacement)?.key, www.key);
  assert.equal(landingRoute(routesOf(app), "https://web.home.lan/api/v1")?.key, api.key);
  assert.equal(landingRoute(routesOf(app), "https://elsewhere.example.com/"), undefined);
});

test("an External app is named after its subdomain, with a suffix when the name is taken", () => {
  const taken = [{ name: "router" }, { name: "router-2" }, { name: "nas" }];
  assert.equal(nameFor("unifi", taken), "unifi");
  assert.equal(nameFor("router", taken), "router-3");
  assert.equal(nameFor("nas.example.com", taken), "nas-2");
});

test("an External app's target drops the scheme's default port", () => {
  const external = (upstream) => ({ id: "x", name: "x", hostname: "x.home.lan", runtime: { kind: "external", upstream } });
  assert.equal(targetLabel(external("https://192.168.1.1:443")), "https://192.168.1.1");
  assert.equal(targetLabel(external("http://192.168.1.5:80")), "http://192.168.1.5");
  assert.equal(targetLabel(external("https://192.168.1.1:8443")), "https://192.168.1.1:8443");
  assert.equal(targetLabel(external("http://[fd00::5]:80")), "http://[fd00::5]");
  assert.equal(targetLabel(external("http://192.168.1.5:443")), "http://192.168.1.5:443");
});

test("a fingerprint typed by hand matches the fetched one whatever its case and colons", () => {
  const fetched = "FA:99:01:F7:AF:22:78:AD:52:C4:FF:B7:B0:13:92:D0:DD:74:77:B0:72:6F:09:76:97:39:DC:6B:A9:C2:D8:9D";
  assert.equal(sameFingerprint(fetched.toLowerCase().replaceAll(":", ""), fetched), true);
  assert.equal(sameFingerprint(` ${fetched} `, fetched), true);
  assert.equal(sameFingerprint(fetched.replace("FA", "FB"), fetched), false);
});
