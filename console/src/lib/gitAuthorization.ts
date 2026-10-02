import type { AuthorizationCompletion, AuthorizationStart, GitProvider } from "./gitConnections.js";

export type AuthorizationPhase = "idle" | "starting" | "waiting" | "completing";
export type AuthorizationPopup = { readonly closed: boolean; close: () => void };
export type AuthorizationMessage = { origin: string; source: unknown; data: unknown };
type MessageTarget = {
  addEventListener: (type: "message", listener: (event: AuthorizationMessage) => void) => void;
  removeEventListener: (type: "message", listener: (event: AuthorizationMessage) => void) => void;
};
type Clock = {
  setTimeout: (callback: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
  setInterval: (callback: () => void, ms: number) => number;
  clearInterval: (id: number) => void;
};

export function callbackFromMessage(event: AuthorizationMessage, origin: string, popup: AuthorizationPopup, state: string): AuthorizationCompletion | null {
  if (event.origin !== origin || event.source !== popup || !state || !event.data || typeof event.data !== "object") return null;
  const value = event.data as Record<string, unknown>;
  if (value.type !== "self-host-git-callback" || value.state !== state) return null;
  if (value.code !== undefined && (typeof value.code !== "string" || !value.code || value.code.length > 8192)) return null;
  if (value.error !== undefined && (typeof value.error !== "string" || !value.error || value.error.length > 256)) return null;
  if (value.installation_id !== undefined && (typeof value.installation_id !== "number" || !Number.isSafeInteger(value.installation_id) || value.installation_id < 1)) return null;
  return {
    state,
    ...(value.code === undefined ? {} : { code: value.code as string }),
    ...(value.installation_id === undefined ? {} : { installation_id: value.installation_id as number }),
    ...(value.error === undefined ? {} : { error: value.error as string }),
  };
}

export function validateConsoleUrl(value: string, origin: string): string {
  let url: URL;
  try { url = new URL(value); } catch { throw new Error("Enter this console's URL, ending in /console/."); }
  if (url.origin !== origin || url.pathname !== "/console/" || url.search || url.hash || url.username || url.password) {
    throw new Error("Use this console's origin and /console/. The authorization popup must return to the same console.");
  }
  const loopback = url.hostname === "localhost" || url.hostname === "127.0.0.1" || url.hostname === "[::1]";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) throw new Error("Provider authentication requires HTTPS, except on localhost.");
  return url.href;
}

export function authorizationDestination(start: AuthorizationStart, provider: GitProvider, registration = false): URL {
  if (!start.state || start.state.length > 4096 || !Number.isFinite(start.expires_in) || start.expires_in <= 0 || start.expires_in > 600) throw new Error("The Host returned an invalid authorization session. Try again.");
  const destination = registration ? start.form?.action : start.url;
  if (!destination || (registration ? Boolean(start.url) : Boolean(start.form))) throw new Error("The Host returned an invalid authorization destination. Try again.");
  let url: URL;
  try { url = new URL(destination); } catch { throw new Error("The Host returned an invalid authorization destination. Try again."); }
  if (url.protocol !== "https:" || url.username || url.password || url.hash || url.searchParams.get("state") !== start.state) throw new Error("The authorization destination is not trusted.");
  const expectedHost = provider === "github" ? "github.com" : "gitlab.com";
  const validPath = registration
    ? provider === "github" && /^\/(?:settings\/apps\/new|organizations\/[A-Za-z0-9-]+\/settings\/apps\/new)$/.test(url.pathname) && typeof start.form?.manifest === "string" && start.form.manifest.length > 0 && start.form.manifest.length < 262144
    : provider === "github" ? /^\/apps\/[A-Za-z0-9-]+\/installations\/new$/.test(url.pathname) || url.pathname === "/login/oauth/authorize" : url.pathname === "/oauth/authorize";
  if (url.hostname !== expectedHost || url.port || !validPath) throw new Error("The authorization destination is not trusted.");
  return url;
}

/** Opens before requesting a session so the browser sees the Operator's click. */
export function beginAuthorization<T>({ openPopup, start, complete, cancel, target, origin, navigate, nextSession, onPhase, clock = {
  setTimeout: (callback, ms) => window.setTimeout(callback, ms),
  clearTimeout: (id) => window.clearTimeout(id),
  setInterval: (callback, ms) => window.setInterval(callback, ms),
  clearInterval: (id) => window.clearInterval(id),
} }: {
  openPopup: () => AuthorizationPopup | null;
  start: () => Promise<AuthorizationStart>;
  complete: (callback: AuthorizationCompletion) => Promise<T>;
  cancel: (state: string) => Promise<void>;
  target: MessageTarget;
  origin: string;
  navigate: (popup: AuthorizationPopup, session: AuthorizationStart) => void;
  nextSession?: (result: T) => AuthorizationStart | null;
  onPhase?: (phase: AuthorizationPhase) => void;
  clock?: Clock;
}) {
  let popup: AuthorizationPopup | null;
  try { popup = openPopup(); } catch { popup = null; }
  if (!popup) return { promise: Promise.reject<T>(new Error("The popup was blocked. Allow popups for this console and try again.")), cancel: (_force = false) => Promise.resolve() };
  const openedPopup = popup;
  let state = "";
  let settled = false;
  let completing = false;
  let timeout: number | undefined;
  let interval: number | undefined;
  let revocation: Promise<void> | undefined;
  let resolve!: (value: T) => void;
  let reject!: (cause: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });

  function revoke() {
    if (state && !revocation) revocation = cancel(state).catch(() => {});
    return revocation ?? Promise.resolve();
  }
  function cleanup(close = true) {
    target.removeEventListener("message", message);
    if (timeout !== undefined) clock.clearTimeout(timeout);
    if (interval !== undefined) clock.clearInterval(interval);
    if (close) { try { openedPopup.close(); } catch { /* A closed popup needs no further action. */ } }
  }
  function fail(cause: unknown) {
    if (settled) return;
    settled = true;
    cleanup();
    void revoke();
    onPhase?.("idle");
    reject(cause instanceof Error ? cause : new Error("Could not authorize Git access. Try again."));
  }
  function message(event: AuthorizationMessage) {
    if (settled || completing) return;
    const callback = callbackFromMessage(event, origin, openedPopup, state);
    if (!callback) return;
    completing = true;
    cleanup(false);
    onPhase?.("completing");
    timeout = clock.setTimeout(() => fail(new Error("Authorization confirmation timed out. Try again.")), 60000);
    // The authenticated completion consumes denied and malformed provider
    // responses too. A bridge message never creates a Connection by itself.
    void complete(callback).then((result) => {
      if (settled) {
        const abandoned = nextSession?.(result);
        if (abandoned?.state) void cancel(abandoned.state).catch(() => {});
        return;
      }
      if (callback.error) { fail(new Error("Access was denied. Try again when you are ready to authorize it.")); return; }
      const next = nextSession?.(result);
      if (next) {
        if (next.state === state) throw new Error("The Host returned a reused authorization session. Try again.");
        completing = false;
        target.addEventListener("message", message);
        watchPopup();
        acceptSession(next);
        return;
      }
      settled = true;
      cleanup();
      onPhase?.("idle");
      resolve(result);
    }).catch((cause) => fail(callback.error ? new Error("Access was denied. Try again when you are ready to authorize it.") : cause));
  }

  function watchPopup() {
    interval = clock.setInterval(() => {
      try { if (openedPopup.closed) fail(new Error("The authorization popup was closed. Try again.")); }
      catch { fail(new Error("The authorization popup could not return to this console. Try again.")); }
    }, 500);
  }
  function acceptSession(session: AuthorizationStart) {
    state = session.state;
    revocation = undefined;
    if (settled) { void revoke(); return; }
    if (!state || state.length > 4096 || !Number.isFinite(session.expires_in) || session.expires_in <= 0 || session.expires_in > 600) throw new Error("The Host returned an invalid authorization session. Try again.");
    if (timeout !== undefined) clock.clearTimeout(timeout);
    timeout = clock.setTimeout(() => fail(new Error("Authorization timed out. Try again.")), session.expires_in * 1000);
    navigate(openedPopup, session);
    onPhase?.("waiting");
  }

  target.addEventListener("message", message);
  onPhase?.("starting");
  timeout = clock.setTimeout(() => fail(new Error("Authorization timed out. Try again.")), 600000);
  watchPopup();
  void Promise.resolve().then(start).then(acceptSession).catch(fail);

  return { promise, cancel: (force = false) => {
    if (completing && !force) return Promise.resolve();
    if (!settled) fail(new Error("Authorization canceled."));
    return revocation ?? Promise.resolve();
  } };
}
