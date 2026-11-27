import { useState } from "react";
import {
  Button, ListPage, Search, StatusBadge, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { StatusFilter } from "../components/StatusFilter.js";
import { Activity, Usage, plainClick } from "../components/WorkloadCells.js";
import { formatBytes } from "../lib/format.js";
import { countOf, machineFailed } from "../lib/overview.js";
import { statusLabel, statusTone } from "../lib/status.js";
import type { Environment, Metrics } from "../lib/types.js";
import { machineSeriesFor } from "../lib/useMetrics.js";

export function Machines({ machines, ready, metrics, onCreate, onOpen }: {
  machines: Environment[];
  ready: boolean;
  metrics: Metrics | null;
  onCreate: () => void;
  onOpen: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const term = query.trim().toLowerCase();
  const visible = machines.filter((one) =>
    one.config.name.toLowerCase().includes(term) && (status === "all" || (status === "failed" ? machineFailed(one) : one.state === status)),
  );
  const filtering = term !== "" || status !== "all";

  return (
    <ListPage title="Machines" action={{ label: "Create virtual machine", icon: <Icon name="plus" />, onClick: onCreate }}
      state={!ready ? "loading" : machines.length ? "ready" : "empty"}
      empty={{ title: "No virtual machines yet", text: "Create a persistent workspace with your tools and repositories, reachable over SSH." }}
      count={`${visible.length} of ${countOf(machines.length, "virtual machine")}`}
      filters={<div className="list-filters">
        <Search aria-label="Search virtual machines" placeholder="Search machines..." value={query} onChange={(event) => setQuery(event.target.value)} />
        <StatusFilter label="Filter virtual machine state" value={status} onChange={setStatus} />
        {filtering ? <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setStatus("all"); }}>Clear filters</Button> : null}
      </div>}>
      <TableHeader><TableRow>
        <TableHead scope="col">Name</TableHead>
        <TableHead scope="col">State</TableHead>
        <TableHead scope="col" className="wide-only">Service</TableHead>
        <TableHead scope="col">Activity</TableHead>
        <TableHead scope="col" className="wide-only">CPU</TableHead>
        <TableHead scope="col" className="wide-only">Memory</TableHead>
      </TableRow></TableHeader>
      <TableBody>
        {!visible.length ? <TableRow><TableCell colSpan={6}>
          No virtual machines match your filters.
        </TableCell></TableRow> : visible.map((one) => {
          const samples = machineSeriesFor(metrics, one.id);
          const creating = one.operation?.status === "running" && one.operation.action === "create" && !one.service_ready;
          return <TableRow key={one.id} className="row-link" onClick={(event) => {
            if ((event.target as Element).closest("a, button")) return;
            onOpen(one.id);
          }}>
            <TableCell>
              <a className="fg" href={`/console/#virtual-machine-${one.id}`} onClick={(event) => {
                if (!plainClick(event)) return;
                event.preventDefault();
                onOpen(one.id);
              }}><strong>{one.config.name}</strong></a>
            </TableCell>
            <TableCell><StatusBadge tone={statusTone(one.state)}>{statusLabel(one.state)}</StatusBadge></TableCell>
            <TableCell className="wide-only">
              {creating ? <span className="muted">—</span>
                : <StatusBadge tone={one.service_ready ? "success" : "neutral"}>{one.service_ready ? "Ready" : "Disabled"}</StatusBadge>}
            </TableCell>
            <TableCell>
              {one.operation?.status === "running" || one.operation?.status === "failed"
                ? <Activity operation={one.operation} />
                : <span className="muted">Idle</span>}
            </TableCell>
            <Usage className="wide-only" samples={samples}
              value={(sample) => sample.cpu_percent} render={(sample) => `${sample.cpu_percent.toFixed(1)}%`} />
            <Usage className="wide-only" samples={samples}
              value={(sample) => sample.memory_bytes} render={(sample) => formatBytes(sample.memory_bytes)} />
          </TableRow>;
        })}
      </TableBody>
    </ListPage>
  );
}
