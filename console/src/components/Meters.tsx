import { Sparkline } from "./Sparkline.js";
import { formatBytes } from "../lib/format.js";
import type { Totals } from "../lib/useMetrics.js";

/**
 * CPU and memory as fractions of what the Host has, and network as a line
 * (ADR-0020). A ratio against a known ceiling is a meter, not a chart: "203
 * MiB of 15.7 GiB" printed as prose is a number nobody reads as 1.3%.
 * Network has no ceiling to measure against, so it keeps its shape instead.
 */
export function Meters({ totals, network }: { totals: Totals; network: number[] }) {
  return (
    <dl className="meters">
      <Meter
        label="CPU"
        fraction={totals.cpu / totals.cpuCapacity}
        value={`${totals.cpu.toFixed(2)}%`}
      />
      <Meter
        label="Memory"
        fraction={totals.memoryLimit ? totals.memory / totals.memoryLimit : 0}
        value={
          <>
            {formatBytes(totals.memory)}
            <span className="muted"> / {formatBytes(totals.memoryLimit)}</span>
          </>
        }
      />
      <div className="meter-row">
        <dt className="meter-label">Network</dt>
        <dd className="meter-plot">
          <Sparkline
            values={network}
            width={240}
            height={22}
            stretch
            label="Network throughput over the collected window"
          />
        </dd>
        <dd className="meter-value">
          ↓ {formatBytes(totals.rx)}
          <span className="muted"> · </span>↑ {formatBytes(totals.tx)}
        </dd>
      </div>
    </dl>
  );
}

function Meter({
  label,
  fraction,
  value,
}: {
  label: string;
  fraction: number;
  value: React.ReactNode;
}) {
  // Clamped, because a Host can be busier than its cores and a bar cannot be
  // longer than its track. The number beside it stays unclamped and true.
  // A fraction that is not a number draws an empty track rather than a full
  // one: `width: NaN%` is invalid CSS, so the browser drops it and a block
  // fills its parent — which is how a daemon that does not report its core
  // count yet showed 0.04% as a bar pinned to 100%.
  const percent = Number.isFinite(fraction) ? Math.min(100, Math.max(0, fraction * 100)) : 0;
  return (
    <div className="meter-row">
      <dt className="meter-label">{label}</dt>
      <dd
        className="meter-track"
        role="meter"
        aria-label={label}
        aria-valuenow={Math.round(percent)}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div className="meter-fill" style={{ width: `${percent}%` }} />
      </dd>
      <dd className="meter-value">{value}</dd>
    </div>
  );
}
