import { api, failureOf } from "./api.js";
import type { App, RedirectRule, Report, RouteRule } from "./types.js";

/** How a route came to be: the Application's Hostname, one of its aliases, or a path rule. */
export type RouteKind = "hostname" | "alias" | "path";

export const kindLabel: Record<RouteKind, string> = {
  hostname: "Hostname",
  alias: "Alias",
  path: "Path",
};

/** The icon that tells each kind apart, on the Routes tab and the Summary diagram. */
export const kindIcon = { hostname: "globe", alias: "alias", path: "split", redirect: "redirect" } as const;

/** One hostname and path the proxy answers, and the Application it reaches. */
export type Route = {
  key: string;
  app: App;
  kind: RouteKind;
  hostname: string;
  path: string;
  stripPrefix: boolean;
  /** A loopback address the rule names instead of the Application's Web Target. */
  target?: string;
};

/** A route as the dialog writes it: what it reaches is the Application it is saved on. */
export type RouteDraft = Pick<Route, "hostname" | "path" | "stripPrefix">;

/** An Application the proxy can reach: it has a Hostname. */
export function isPublished(app: App): boolean {
  return app.publication?.kind !== "unpublished" && !app.managed_postgres && app.hostname !== "";
}

/** The Hostname, its aliases, then the paths routed to the Application. */
export function routesOf(app: App): Route[] {
  if (!isPublished(app)) return [];
  const root = (kind: RouteKind, hostname: string): Route =>
    ({ key: `${app.id}:${kind}:${hostname}`, app, kind, hostname, path: "/", stripPrefix: false });
  return [
    root("hostname", app.hostname),
    ...(app.aliases ?? []).map((alias) => root("alias", alias)),
    ...(app.route_rules ?? []).map((rule): Route => ({
      key: `${app.id}:path:${rule.hostname}${rule.path_prefix}`,
      app,
      kind: "path",
      hostname: rule.hostname,
      path: rule.path_prefix,
      stripPrefix: rule.strip_prefix ?? false,
      target: rule.target ?? undefined,
    })),
  ];
}

export function urlOf(route: { hostname: string; path: string }): string {
  return `https://${route.hostname}${route.path === "/" ? "" : route.path}`;
}

/** Where the Application's Web Target listens, as a route reaches it. */
export function targetOf(app: App): string | undefined {
  if (app.runtime?.kind === "native") return app.runtime.port ? `:${app.runtime.port}` : undefined;
  const port = app.development?.web_port ?? app.web_port ?? 80;
  return app.web_service ? `${app.web_service}:${port}` : `:${port}`;
}

/** The Web Target with its owner named, as a table or a preview reads it: `blog:80`, `web:80`. */
export function targetLabel(app: App): string {
  const target = targetOf(app);
  if (!target) return app.name;
  return target.startsWith(":") ? `${app.name}${target}` : target;
}

/**
 * The aliases and rules an Application keeps once `previous` is replaced by
 * `next`, either of which may be absent. A `/` route is an alias; any other
 * path is a rule. A changed route keeps its place, and a rule keeps the
 * target it named.
 */
export function replaceRoute(app: App, previous: Route | null, next: RouteDraft | null): { aliases: string[]; route_rules: RouteRule[] } {
  const aliases = [...(app.aliases ?? [])];
  const rules = [...(app.route_rules ?? [])];
  const alias = previous?.kind === "alias" ? aliases.indexOf(previous.hostname) : -1;
  const rule = previous?.kind === "path"
    ? rules.findIndex((one) => one.hostname === previous.hostname && one.path_prefix === previous.path)
    : -1;
  if (alias >= 0) aliases.splice(alias, 1);
  if (rule >= 0) rules.splice(rule, 1);
  if (next && next.path === "/") {
    aliases.splice(alias >= 0 ? alias : aliases.length, 0, next.hostname);
  } else if (next) {
    const target = previous?.target ? { target: previous.target } : {};
    rules.splice(rule >= 0 ? rule : rules.length, 0, { hostname: next.hostname, path_prefix: next.path, strip_prefix: next.stripPrefix, ...target });
  }
  return { aliases, route_rules: rules };
}

/** Whether the Application sends its Web Target's address as `Host`; null follows the setting. Nothing is rebuilt or restarted. */
export async function saveRewriteHost(app: App, rewriteHost: boolean | null): Promise<Report | null> {
  const response = await api(`/apps/id/${encodeURIComponent(app.id)}/routes`, {
    method: "PUT",
    body: JSON.stringify({ rewrite_host: rewriteHost }),
  });
  return response.ok ? null : failureOf(response);
}

/** Saves an Application's redirects, in the order they are tried. Nothing is rebuilt or restarted. */
export async function saveRedirects(app: App, redirects: RedirectRule[]): Promise<Report | null> {
  const response = await api(`/apps/id/${encodeURIComponent(app.id)}/routes`, {
    method: "PUT",
    body: JSON.stringify({ redirects }),
  });
  return response.ok ? null : failureOf(response);
}

/** Saves an Application's aliases and rules, and its redirects when given. Nothing is rebuilt or restarted; the failure comes back as a report. */
export async function saveRoutes(app: App, routes: { aliases: string[]; route_rules: RouteRule[]; redirects?: RedirectRule[] }): Promise<Report | null> {
  const response = await api(`/apps/id/${encodeURIComponent(app.id)}/routes`, {
    method: "PUT",
    body: JSON.stringify(routes),
  });
  return response.ok ? null : failureOf(response);
}

const escape = (text: string) => text.replace(/\./g, "\\.");

/** A redirect from one hostname and path, as the route dialog writes it. A location on the same hostname is its path. */
export type RedirectDraft = { hostname: string; path: string; location: string; permanent: boolean };

/** The rule for a draft: the exact path, with or without a trailing slash, on that hostname only. */
export function redirectRule(draft: RedirectDraft): RedirectRule {
  const path = draft.path === "/" ? "" : draft.path.replace(/\/+$/, "");
  return {
    pattern: `^https?://${escape(draft.hostname)}${escape(path)}/?$`,
    replacement: draft.location.startsWith("/") ? `https://${draft.hostname}${draft.location}` : draft.location,
    permanent: draft.permanent,
  };
}

/** The draft behind a rule `redirectRule` wrote, or null for a pattern written by hand. */
export function redirectDraft(rule: RedirectRule): RedirectDraft | null {
  const match = /^\^https\?:\/\/((?:[a-z0-9-]+\\\.)*[a-z0-9-]+)((?:\/(?:[A-Za-z0-9_~-]|\\\.)+)*)\/\?\$$/.exec(rule.pattern);
  if (!match || /[\s$]/.test(rule.replacement)) return null;
  const hostname = match[1].replace(/\\\./g, ".");
  const own = `https://${hostname}/`;
  return {
    hostname,
    path: match[2].replace(/\\\./g, ".") || "/",
    location: rule.replacement.startsWith(own) ? rule.replacement.slice(own.length - 1) : rule.replacement,
    permanent: rule.permanent ?? false,
  };
}

/** What a redirect answers, as a table or the diagram reads it: a URL when it has one, else its pattern. */
export function redirectFrom(rule: RedirectRule): string {
  const draft = redirectDraft(rule);
  return draft ? `${draft.hostname}${draft.path}` : rule.pattern;
}

/** Where a redirect sends, as a table or the diagram reads it: a path on the same hostname, or the location. */
export function redirectTo(rule: RedirectRule): string {
  return redirectDraft(rule)?.location ?? rule.replacement;
}

/** The route a redirect's location arrives at: its hostname, and the longest path prefix that holds it. */
export function landingRoute(routes: Route[], location: string): Route | undefined {
  let url: URL;
  try { url = new URL(location); } catch { return undefined; }
  return routes
    .filter((route) => route.hostname === url.hostname && (route.path === "/" || url.pathname === route.path || url.pathname.startsWith(`${route.path}/`)))
    .sort((a, b) => b.path.length - a.path.length)[0];
}
