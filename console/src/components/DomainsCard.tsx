import { useState } from "react";

import { kindIcon, redirectFrom, redirectTo, routesOf, targetLabel } from "../lib/routes.js";
import type { App } from "../lib/types.js";
import { Icon, type IconName } from "./Icon.js";

/** The kinds the card groups by, and what adding one starts with. */
type Group = "domains" | "paths" | "redirects";
type Line = { key: string; icon: IconName; url: string; to: string };

const titles: Record<Group, string> = { domains: "Hostnames", paths: "Paths", redirects: "Redirects" };
const groupIcon: Record<Group, IconName> = { domains: kindIcon.hostname, paths: kindIcon.path, redirects: kindIcon.redirect };

/**
 * Every way into an Application, as the Summary diagram shows it: the
 * hostnames, the paths and the redirects, each a section with its own add
 * action. The header folds the card, and once more than one kind is there,
 * chips show one kind at a time.
 */
export function DomainsCard({ app, onAdd }: {
  app: App;
  /** Opens the route dialog set for that kind. */
  onAdd: (group: Group) => void;
}) {
  const [open, setOpen] = useState(true);
  const [only, setOnly] = useState<Group | null>(null);
  const target = targetLabel(app);
  const routes = routesOf(app);
  const lines: Record<Group, Line[]> = {
    domains: routes.filter((route) => route.kind !== "path").map((route) => ({
      key: route.key, icon: kindIcon[route.kind], url: `${route.hostname}/`, to: `→ ${target}`,
    })),
    paths: routes.filter((route) => route.kind === "path").map((route) => ({
      key: route.key, icon: kindIcon.path, url: `${route.hostname}${route.path}`,
      to: `→ ${route.target ?? target}${route.stripPrefix ? "" : route.path}`,
    })),
    redirects: (app.redirects ?? []).map((rule, index) => ({
      key: `redirect:${index}`, icon: kindIcon.redirect, url: redirectFrom(rule),
      to: `↪ ${redirectTo(rule)} (${rule.permanent ? "301" : "302"})`,
    })),
  };
  const groups = (Object.keys(lines) as Group[]).filter((group) => lines[group].length > 0);
  const total = groups.reduce((count, group) => count + lines[group].length, 0);
  // A filter that has nothing left to show falls back to all of them.
  const shown = only && groups.includes(only) && groups.length > 1 ? [only] : groups;

  return (
    <span className="domains-card">
      <button type="button" className="domains-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span>Domains <span className="domains-count">{total}</span></span>
        <Icon name={open ? "chevron-up" : "chevron-down"} />
      </button>
      {open && groups.length > 1 ? (
        <span className="domains-chips" role="group" aria-label="Show">
          <button type="button" aria-pressed={shown.length > 1} onClick={() => setOnly(null)}>All {total}</button>
          {groups.map((group) => (
            <button key={group} type="button" aria-pressed={shown.length === 1 && shown[0] === group} onClick={() => setOnly(group)}>
              {titles[group]} {lines[group].length}
            </button>
          ))}
        </span>
      ) : null}
      {open ? shown.map((group) => (
        <span key={group} className="domains-section">
          <span className="domains-section-head">
            <span><Icon name={groupIcon[group]} />{titles[group]} <span className="domains-count">{lines[group].length}</span></span>
            <button type="button" className="domains-add" aria-label={`Add to ${titles[group]}`} onClick={() => onAdd(group)}><Icon name="plus" /></button>
          </span>
          {lines[group].map((line) => (
            <span key={line.key} className="domains-line">
              <Icon name={line.icon} />
              <span className="mono">{line.url}</span>
              <span className="mono muted">{line.to}</span>
            </span>
          ))}
        </span>
      )) : null}
    </span>
  );
}
