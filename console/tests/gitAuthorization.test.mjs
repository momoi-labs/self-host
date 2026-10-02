import assert from "node:assert/strict";
import { test } from "node:test";
import { authorizationDestination, beginAuthorization, callbackFromMessage, validateConsoleUrl } from "../src/lib/gitAuthorization.ts";

const origin = "https://console.example.test";
const session = (state = "state-install") => ({ state, url: `https://github.com/apps/self-host/installations/new?state=${state}`, form: null, expires_in: 600 });
const flush = () => new Promise((resolve) => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function harness(options = {}) {
  const popup = { closed: false, closes: 0, close() { this.closed = true; this.closes++; } };
  const listeners = new Set();
  const timeouts = new Map();
  const intervals = new Map();
  const canceled = [], completed = [], navigated = [], phases = [];
  let sequence = 0, starts = 0;
  const clock = {
    setTimeout: (callback, ms) => { const id = ++sequence; timeouts.set(id, { callback, ms }); return id; },
    clearTimeout: (id) => timeouts.delete(id),
    setInterval: (callback, ms) => { const id = ++sequence; intervals.set(id, { callback, ms }); return id; },
    clearInterval: (id) => intervals.delete(id),
  };
  const handle = beginAuthorization({
    openPopup: () => options.blocked ? null : popup,
    start: () => { starts++; return options.start?.() ?? Promise.resolve(session()); },
    complete: async (callback) => { completed.push(callback); return options.complete?.(callback) ?? { connection: { id: "saved" }, authorization: null }; },
    cancel: async (state) => { canceled.push(state); },
    target: { addEventListener: (_, listener) => listeners.add(listener), removeEventListener: (_, listener) => listeners.delete(listener) },
    origin,
    navigate: (window, next) => { assert.equal(window, popup); navigated.push(next); },
    nextSession: (result) => result.authorization ?? null,
    onPhase: (phase) => phases.push(phase),
    clock,
  });
  function message(data, overrides = {}) {
    for (const listener of [...listeners]) listener({ origin, source: popup, data, ...overrides });
  }
  function timeout(ms) {
    const timer = [...timeouts.values()].find((item) => item.ms === ms);
    assert.ok(timer, `Missing ${ms}ms timer`);
    timer.callback();
  }
  return { popup, listeners, timeouts, intervals, canceled, completed, navigated, phases, handle, message, timeout, starts: () => starts };
}

test("the bridge accepts only the expected origin, popup and state and strips extra fields", () => {
  const popup = { closed: false, close() {} };
  const event = { origin, source: popup, data: { type: "self-host-git-callback", state: "known", code: "provider-code", installation_id: 42, api_key: "must-not-forward", token: "must-not-forward" } };
  assert.deepEqual(callbackFromMessage(event, origin, popup, "known"), { state: "known", code: "provider-code", installation_id: 42 });
  for (const changed of [
    { ...event, origin: "https://untrusted.example.test" },
    { ...event, source: {} },
    { ...event, data: { ...event.data, state: "other" } },
    { ...event, data: { ...event.data, installation_id: "42" } },
    { ...event, data: { ...event.data, installation_id: 1.5 } },
    { ...event, data: { ...event.data, code: "" } },
    { ...event, data: null },
  ]) assert.equal(callbackFromMessage(changed, origin, popup, "known"), null);
});

test("a blocked popup never creates a server session", async () => {
  const run = harness({ blocked: true });
  await assert.rejects(run.handle.promise, /popup was blocked/);
  assert.equal(run.starts(), 0);
  assert.equal(run.listeners.size, 0);
  assert.deepEqual(run.canceled, []);
});

test("a forged callback cannot complete authorization and a verified callback completes only once", async () => {
  const outcome = deferred();
  const run = harness({ complete: () => outcome.promise });
  await flush();
  const callback = { type: "self-host-git-callback", state: "state-install", code: "code" };
  run.message(callback, { origin: "https://untrusted.example.test" });
  run.message(callback, { source: {} });
  run.message({ ...callback, state: "wrong" });
  assert.deepEqual(run.completed, []);
  run.message(callback);
  run.message(callback);
  assert.equal(run.completed.length, 1);
  assert.equal(run.intervals.size, 0);
  outcome.resolve({ connection: { id: "saved" }, authorization: null });
  assert.deepEqual(await run.handle.promise, { connection: { id: "saved" }, authorization: null });
  assert.equal(run.popup.closed, true);
  assert.equal(run.listeners.size, 0);
  assert.equal(run.timeouts.size, 0);
  assert.deepEqual(run.canceled, []);
});

test("canceling before the session arrives revokes the late session without navigating", async () => {
  const started = deferred();
  const run = harness({ start: () => started.promise });
  const rejected = assert.rejects(run.handle.promise, /canceled/);
  await run.handle.cancel();
  await rejected;
  started.resolve(session("late-state"));
  await flush();
  assert.deepEqual(run.canceled, ["late-state"]);
  assert.deepEqual(run.navigated, []);
  assert.equal(run.popup.closed, true);
});

test("closing or timing out the popup revokes its pending state and removes listeners", async () => {
  for (const reason of ["closed", "timeout"]) {
    const run = harness();
    await flush();
    const rejected = assert.rejects(run.handle.promise, reason === "closed" ? /popup was closed/ : /timed out/);
    if (reason === "closed") {
      run.popup.closed = true;
      [...run.intervals.values()][0].callback();
    } else run.timeout(600000);
    await rejected;
    assert.deepEqual(run.canceled, ["state-install"]);
    assert.equal(run.listeners.size, 0);
    assert.equal(run.timeouts.size, 0);
    assert.equal(run.intervals.size, 0);
  }
});

test("denied callbacks are consumed through authenticated completion and then report denial", async () => {
  const run = harness({ complete: () => { throw new Error("Provider detail must not replace the denial message"); } });
  await flush();
  const rejected = assert.rejects(run.handle.promise, /Access was denied/);
  run.message({ type: "self-host-git-callback", state: "state-install", error: "access_denied" });
  await rejected;
  assert.deepEqual(run.completed, [{ state: "state-install", error: "access_denied" }]);
  assert.deepEqual(run.canceled, ["state-install"]);
  assert.equal(run.popup.closed, true);
});

test("GitHub installation and OAuth use the same popup and reject callbacks from the previous stage", async () => {
  const next = { ...session("state-oauth"), url: "https://github.com/login/oauth/authorize?state=state-oauth&code_challenge=challenge&code_challenge_method=S256" };
  const run = harness({ complete: (callback) => callback.code ? { connection: { id: "existing", credential_id: "stable" }, authorization: null } : { connection: null, authorization: next } });
  await flush();
  run.message({ type: "self-host-git-callback", state: "state-install", installation_id: 42 });
  await flush();
  assert.equal(run.popup.closed, false);
  assert.deepEqual(run.navigated.map((item) => item.state), ["state-install", "state-oauth"]);
  run.message({ type: "self-host-git-callback", state: "state-install", code: "stale-code" });
  assert.equal(run.completed.length, 1);
  run.message({ type: "self-host-git-callback", state: "state-oauth", code: "verified-code" });
  const result = await run.handle.promise;
  assert.equal(result.connection.credential_id, "stable");
  assert.deepEqual(run.completed, [{ state: "state-install", installation_id: 42 }, { state: "state-oauth", code: "verified-code" }]);
});

test("a timed out or unmounted completion cancels any late continuation state", async () => {
  for (const reason of ["timeout", "unmount"]) {
    const complete = deferred();
    const run = harness({ complete: () => complete.promise });
    await flush();
    run.message({ type: "self-host-git-callback", state: "state-install", installation_id: 42 });
    const rejected = assert.rejects(run.handle.promise, reason === "timeout" ? /confirmation timed out/ : /canceled/);
    if (reason === "timeout") run.timeout(60000);
    else await run.handle.cancel(true);
    await rejected;
    complete.resolve({ connection: null, authorization: session("orphan-state") });
    await flush();
    assert.deepEqual(run.canceled, ["state-install", "orphan-state"]);
    assert.equal(run.navigated.length, 1);
    assert.equal(run.listeners.size, 0);
    assert.equal(run.timeouts.size, 0);
  }
});

test("an invalid session expiry is rejected and its state is revoked", async () => {
  const run = harness({ start: () => Promise.resolve({ ...session(), expires_in: 601 }) });
  await assert.rejects(run.handle.promise, /invalid authorization session/);
  assert.deepEqual(run.canceled, ["state-install"]);
  assert.equal(run.popup.closed, true);
});

test("only official provider destinations with the expected state can receive authorization data", () => {
  assert.equal(authorizationDestination(session(), "github").hostname, "github.com");
  assert.equal(authorizationDestination({ ...session(), url: "https://gitlab.com/oauth/authorize?state=state-install" }, "gitlab").hostname, "gitlab.com");
  assert.equal(authorizationDestination({ ...session(), url: null, form: { action: "https://github.com/organizations/team/settings/apps/new?state=state-install", manifest: "{}" } }, "github", true).hostname, "github.com");
  for (const url of [
    "https://github.com.evil.example/apps/self-host/installations/new?state=state-install",
    "http://github.com/apps/self-host/installations/new?state=state-install",
    "https://user:password@github.com/apps/self-host/installations/new?state=state-install",
    "https://github.com:444/apps/self-host/installations/new?state=state-install",
    "https://github.com/apps/self-host/installations/new?state=wrong",
    "https://github.com/logout?state=state-install",
  ]) assert.throws(() => authorizationDestination({ ...session(), url }, "github"), /not trusted/);
});

test("setup URLs stay on the current console origin and require HTTPS except for loopback", () => {
  assert.equal(validateConsoleUrl(`${origin}/console/`, origin), `${origin}/console/`);
  assert.equal(validateConsoleUrl("http://127.0.0.1:23722/console/", "http://127.0.0.1:23722"), "http://127.0.0.1:23722/console/");
  for (const value of ["https://other.example.test/console/", `${origin}/elsewhere`, `${origin}/console/?api_key=secret`, `${origin}/console/#new`, "https://user:secret@console.example.test/console/"]) assert.throws(() => validateConsoleUrl(value, origin));
  assert.throws(() => validateConsoleUrl("http://192.168.1.20/console/", "http://192.168.1.20"), /requires HTTPS/);
});
