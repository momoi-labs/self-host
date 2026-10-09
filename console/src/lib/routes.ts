import { api, failureOf } from "./api.js";
import type { App, Report, RouteRule } from "./types.js";

/** How a route came to be: the Application's Hostname, one of its aliases, or a path rule. */
export type RouteKind = "hostname" | "alias" | "path";

export const kindLabel: Record<RouteKind, string> = {
  hostname: "Hostname",
  alias: "Alias",
  path: "Path",
};

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

/** Saves an Application's aliases and rules. Nothing is rebuilt or restarted; the failure comes back as a report. */
export async function saveRoutes(app: App, routes: { aliases: string[]; route_rules: RouteRule[] }): Promise<Report | null> {
  const response = await api(`/apps/id/${encodeURIComponent(app.id)}/routes`, {
    method: "PUT",
    body: JSON.stringify(routes),
  });
  return response.ok ? null : failureOf(response);
}
