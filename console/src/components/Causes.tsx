import type { Report } from "../lib/types.js";

/**
 * One layer per line, in the order the platform stacked them (ADR-0010).
 * Naming the layers is the whole point: the reader can tell the Platform's
 * framing apart from what Docker said.
 */
export function Causes({ report }: { report: Report }) {
  if (!report.caused_by.length) return null;
  return (
    <ol className="alert-causes">
      {report.caused_by.map((cause, i) => (
        <li key={i}>{cause}</li>
      ))}
    </ol>
  );
}
