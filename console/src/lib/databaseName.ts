/**
 * Consumer database names, checked the way the daemon checks them: a plain
 * lowercase PostgreSQL identifier that PostgreSQL does not keep for itself.
 */
export function validDatabaseName(name: string): boolean {
  return /^[a-z_][a-z0-9_]{0,62}$/.test(name)
    && !name.startsWith("pg_")
    && !["postgres", "template0", "template1"].includes(name);
}

/** The database a new connection gets by default, named after its Application: `mochi-cut` becomes `mochi_cut`. */
export function defaultDatabaseName(application: string): string {
  let name = application.toLowerCase().replace(/[^a-z0-9_]+/g, "_").replace(/_+/g, "_").replace(/^_+|_+$/g, "");
  if (!/^[a-z]/.test(name) || !validDatabaseName(name)) name = `db_${name}`.replace(/_+$/, "");
  return name.slice(0, 63).replace(/_+$/, "");
}
