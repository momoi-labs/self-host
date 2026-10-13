/** Formats bytes the way the console speaks: binary units, one decimal. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = -1;
  do {
    value /= 1024;
    unit += 1;
  } while (value >= 1024 && unit < units.length - 1);
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

/** How long a stretch of samples covers, in the coarsest honest unit. */
export function formatWindow(seconds: number): string {
  if (seconds < 90) return `${Math.round(seconds)}s`;
  if (seconds < 90 * 60) return `${Math.round(seconds / 60)} min`;
  return `${(seconds / 3600).toFixed(1)}h`;
}

/** How long ago an instant was, in the coarsest unit that still says something: "9 minutes ago". */
export function formatAgo(iso: string, now: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - Date.parse(iso)) / 1000));
  const steps: [number, string][] = [[60, "second"], [60, "minute"], [24, "hour"], [30, "day"], [12, "month"]];
  let value = seconds;
  for (const [size, unit] of steps) {
    if (value < size) return value <= 1 && unit === "second" ? "just now" : `${value} ${unit}${value === 1 ? "" : "s"} ago`;
    value = Math.floor(value / size);
  }
  return `${value} year${value === 1 ? "" : "s"} ago`;
}
