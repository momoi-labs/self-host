import type { ImageDependency } from "./devImageTemplates.js";

/** What mise resolves a tool by: a short name, or `backend:name`. */
const KEY = /^[a-zA-Z0-9][a-zA-Z0-9._:/@-]{0,159}$/;

/** What mise accepts as a version, including `latest` and a bare major. */
const VERSION = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$/;

/** The only option a dependency carries here. mise reads it on npm-backed
 * tools, where it names the packages whose install scripts may run. */
export const ALLOW_BUILDS = "allow_builds";

export function isKey(key: string): boolean {
  return KEY.test(key);
}

export function isVersion(version: string): boolean {
  return VERSION.test(version);
}

/** Whether mise would read `allow_builds` on this tool at all. Offering the
 * option on `node` would invite an entry the build then ignores. */
export function takesAllowBuilds(tool: string): boolean {
  return tool.startsWith("npm:");
}

/** The tool split the way a chip reads it: `npm:t3` is `t3` installed by
 * `npm`, while `node` is one segment because it has no backend to name. */
export function splitKey(tool: string): { scope?: string; name: string } {
  const colon = tool.indexOf(":");
  if (colon < 0) return { name: tool };
  return { scope: tool.slice(0, colon), name: tool.slice(colon + 1) };
}

/**
 * What to offer for a query, best first: an exact key, then a prefix, then
 * anything containing it. Backend keys come out of each tool's own list, so
 * `t3` reaches `npm:t3` without the catalog naming it twice.
 *
 * The query itself leads the list when it is a key mise could resolve and the
 * catalog has nothing better, which is what keeps entry working while the
 * search is unavailable.
 */
export function suggest(
  tools: { name: string; backends: string[] }[],
  query: string,
  taken: string[],
  limit = 20,
): string[] {
  const term = query.toLowerCase().trim();
  const held = new Set(taken);
  const keys = new Set(tools.flatMap((tool) => [
    tool.name,
    ...tool.backends.filter((backend) => backend.includes(":")),
  ]));
  const rank = (key: string) =>
    key.toLowerCase() === term ? 0 : key.toLowerCase().startsWith(term) ? 1 : 2;
  const matches = [...keys]
    .filter((key) => key.toLowerCase().includes(term) && !held.has(key))
    .sort((a, b) => rank(a) - rank(b) || a.localeCompare(b))
    .slice(0, limit);
  const typed = query.trim();
  const direct = isKey(typed) && !held.has(typed) && !matches.includes(typed);
  // A backend key is always the operator's own: the catalog lists tools, not
  // every package a backend can install.
  if (direct && (typed.includes(":") || !matches.length)) matches.unshift(typed);
  return matches;
}

/**
 * One option as the chip prints it back, `name=value` or `name=[a, b]`.
 * ChipInput never parses, so the brackets and the separator are read here.
 * An option with no values left is a removal.
 */
export function readOption(text: string): { name: string; values: string[] } | null {
  const equals = text.indexOf("=");
  if (equals < 0) return null;
  const name = text.slice(0, equals).trim();
  if (!name) return null;
  const body = text.slice(equals + 1).trim().replace(/^\[/, "").replace(/\]$/, "");
  return { name, values: body.split(",").map((value) => value.trim()).filter(Boolean) };
}

/** The recipe as mise reads it. A tool with options is a table rather than a
 * bare version string. */
export function miseToml(dependencies: ImageDependency[], checks: string[]): string {
  const tools = dependencies.map(({ tool, version, allow_builds }) => {
    const value = allow_builds?.length
      ? `{ version = ${JSON.stringify(version)}, allow_builds = ${JSON.stringify(allow_builds)} }`
      : JSON.stringify(version);
    return `${JSON.stringify(tool)} = ${value}`;
  });
  return `[tools]\n${tools.join("\n")}\n`
    + (checks.length ? `\n[tasks.check]\nrun = ${JSON.stringify(checks)}\n` : "");
}
