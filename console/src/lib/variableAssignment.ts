/**
 * Reads one `NAME=value` line, as copied from a `.env` file or a shell, into
 * its name and value. An optional `export` is dropped, and so is one pair of
 * matching quotes around the value. Anything else, including several lines,
 * is not an assignment and returns null.
 */
export function variableAssignment(text: string): { name: string; value: string } | null {
  const match = /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*?)\r?\n?$/.exec(text);
  if (!match || match[2].includes("\n")) return null;
  const value = match[2];
  const quoted = value.length >= 2 && (value[0] === '"' || value[0] === "'") && value.at(-1) === value[0];
  return { name: match[1], value: quoted ? value.slice(1, -1) : value };
}
