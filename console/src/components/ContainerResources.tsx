import { Table, TableBody, TableCell, TableFooter, TableHead, TableHeader, TableRow } from "@momoi-labs/kiso-react";

import { Meters } from "./Meters.js";
import { Sparkline } from "./Sparkline.js";
import { StatusBadge } from "./StatusBadge.js";
import { formatBytes, formatWindow } from "../lib/format.js";
import { serviceTone } from "../lib/status.js";
import type { AppSample, ContainerSeries, ServiceState } from "../lib/types.js";
import type { Totals } from "../lib/useMetrics.js";

type Row = {
  key: string;
  /** The Compose service name, or the container's when there is no Compose. */
  name: string;
  state?: string;
  exitCode?: number;
  samples: AppSample[] | null;
};

/**
 * What this Application is using, and which of its containers is using it
 * (ADR-0020). One table, not two: the Services list and the per-container
 * metrics are the same rows, and printing them apart meant printing every
 * one-container Application twice.
 */
export function ContainerResources({
  samples,
  services,
  containers,
  totals,
  intervalSeconds,
}: {
  /** The Application's own series, already summed across its containers. */
  samples: AppSample[] | null;
  services: ServiceState[];
  containers: ContainerSeries[];
  totals: Totals | null;
  intervalSeconds: number;
}) {
  const metrics = new Map(containers.map((series) => [series.container, series.samples]));
  // Docker's list is the authoritative one — a stopped container still has a
  // row, it just has no numbers. Anything Docker reports but the record does
  // not name is still shown rather than dropped.
  const named = new Set(services.map((service) => service.container));
  const rows: Row[] = [
    ...services.map((service) => ({
      key: service.container,
      name: service.service || service.container,
      state: service.state,
      exitCode: service.exit_code,
      samples: metrics.get(service.container) ?? null,
    })),
    ...containers
      .filter((series) => !named.has(series.container))
      .map((series) => ({
        key: series.container,
        name: series.container,
        state: undefined,
        exitCode: undefined,
        samples: series.samples,
      })),
  ];

  const window = rows.reduce((most, row) => Math.max(most, row.samples?.length ?? 0), 0);

  if (!totals && rows.length === 0) {
    return <p className="muted">No samples yet. The first lands one tick after the deploy.</p>;
  }

  return (
    <div className="resources">
      {totals ? (
        <Meters
          totals={totals}
          network={(samples ?? []).map((sample) => sample.rx_bytes + sample.tx_bytes)}
        />
      ) : null}

      {rows.length ? (
        <div className="container-metrics">
          <p className="t-caps">Per container</p>
          <div className="table-wrap">
            <div className="table-scroll">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Service</TableHead>
                    <TableHead scope="col">State</TableHead>
                    <TableHead scope="col" className="num">
                      CPU
                    </TableHead>
                    <TableHead scope="col" className="num">
                      Memory
                    </TableHead>
                    <TableHead scope="col" className="num">
                      {window ? formatWindow(window * intervalSeconds) : "Trend"}
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => {
                    const latest = row.samples?.[row.samples.length - 1];
                    return (
                      <TableRow key={row.key}>
                        <TableCell className="mono">{row.name}</TableCell>
                        <TableCell>
                          {row.state ? (
                            <StatusBadge tone={serviceTone(row.state)}>
                              {row.state}
                              {row.state === "exited" && row.exitCode !== undefined
                                ? ` (${row.exitCode})`
                                : ""}
                            </StatusBadge>
                          ) : (
                            <span className="muted">—</span>
                          )}
                        </TableCell>
                        <TableCell className="num">
                          {latest ? `${latest.cpu_percent.toFixed(2)}%` : <span className="muted">—</span>}
                        </TableCell>
                        <TableCell className="num">
                          {latest ? formatBytes(latest.memory_bytes) : <span className="muted">—</span>}
                        </TableCell>
                        <TableCell className="num">
                          {row.samples ? (
                            <Sparkline
                              values={row.samples.map((sample) => sample.cpu_percent)}
                              width={70}
                              height={20}
                              label={`CPU of ${row.name} over the collected window`}
                            />
                          ) : null}
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
                {totals && rows.length > 1 ? (
                  <TableFooter>
                    <TableRow>
                      <TableCell colSpan={2}>Application</TableCell>
                      <TableCell className="num">{totals.cpu.toFixed(2)}%</TableCell>
                      <TableCell className="num">{formatBytes(totals.memory)}</TableCell>
                      <TableCell />
                    </TableRow>
                  </TableFooter>
                ) : null}
              </Table>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
