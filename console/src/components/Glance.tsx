import { Sparkline } from "@momoi-labs/kiso-react";

import { formatBytes } from "../lib/format.js";
import type { AppSample } from "../lib/types.js";

/**
 * One line of numbers under a detail screen's name: what it is using, and the
 * shape of the window behind each reading (ADR-0020). The screen's job is
 * configuration and output, so the history stays a line rather than a card
 * that pushes the panel down.
 *
 * An Application sums its containers and a machine measures itself, but both
 * arrive as the same series, so both read the same way here.
 */
export function Glance({ samples }: { samples: AppSample[] }) {
  const latest = samples[samples.length - 1];
  if (!latest) return null;
  return (
    <div className="glance">
      <span>
        <span className="k">CPU</span> <b>{latest.cpu_percent.toFixed(2)}%</b>
      </span>
      <Sparkline values={samples.map((sample) => sample.cpu_percent)} height={18} />
      <span className="sep">·</span>
      <span>
        <span className="k">Memory</span> <b>{formatBytes(latest.memory_bytes)}</b>
      </span>
      <Sparkline values={samples.map((sample) => sample.memory_bytes)} height={18} />
      <span className="sep">·</span>
      <span>
        <span className="k">Net</span>{" "}
        <b>
          <span className="network-down">↓ {formatBytes(latest.rx_bytes)}</span>
          <span className="muted"> · </span>
          <span className="network-up">↑ {formatBytes(latest.tx_bytes)}</span>
        </b>
      </span>
    </div>
  );
}
