/** What a database dump is, as pg_dump writes it. */
export type ArchiveFormat = "custom" | "tar" | "directory" | "sql";

/** What a dump is from its first bytes, as the server tells them apart; null when unsure. */
export function detectFormat(head: Uint8Array): ArchiveFormat | null {
  const text = (from: number, to: number) => String.fromCharCode(...head.slice(from, to));
  if (text(0, 5) === "PGDMP") return "custom";
  if (head.length >= 262 && text(257, 262) === "ustar") return "tar";
  if (head.length && !head.includes(0)) {
    try {
      const start = new TextDecoder("utf-8", { fatal: true }).decode(head).trimStart();
      if (start.startsWith("--") || start.startsWith("SET ") || start.startsWith("CREATE ")) return "sql";
    } catch { /* not text */ }
  }
  return null;
}
