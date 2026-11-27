import { useState } from "react";
import {
  Button, ListPage, Search, StatusBadge, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { definitionOf, describeDefinition } from "../components/DefinitionPicker.js";
import { Icon } from "../components/Icon.js";
import { StatusFilter } from "../components/StatusFilter.js";
import { Usage, plainClick } from "../components/WorkloadCells.js";
import { formatBytes } from "../lib/format.js";
import { countOf, isApplication } from "../lib/overview.js";
import { statusLabel, statusTone } from "../lib/status.js";
import type { App, Metrics } from "../lib/types.js";
import { seriesFor } from "../lib/useMetrics.js";
import { useNativeCapabilities } from "../lib/useNativeCapabilities.js";

function sourceOf(app: App): string {
  if (app.runtime?.kind === "native") return app.git?.repository ?? app.runtime.worker?.config ?? app.runtime.command.join(" ");
  return app.git?.repository ?? app.image;
}

export function Apps({ apps, ready, metrics, onDeploy, onOpen }: {
  apps: App[];
  ready: boolean;
  metrics: Metrics | null;
  onDeploy: () => void;
  onOpen: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const { capabilities } = useNativeCapabilities();
  const applications = apps.filter(isApplication);
  const term = query.trim().toLowerCase();
  const visible = applications.filter((app) =>
    (app.name.toLowerCase().includes(term) || (app.hostname ?? "").toLowerCase().includes(term)) &&
    (status === "all" || app.status === status),
  );
  const filtering = term !== "" || status !== "all";

  return (
    <ListPage title="Apps" action={{ label: "Deploy application", icon: <Icon name="plus" />, onClick: onDeploy }}
      state={!ready ? "loading" : applications.length ? "ready" : "empty"}
      empty={{ title: "No applications yet", text: "Deploy a container image, a Git repository or a native process." }}
      count={`${visible.length} of ${countOf(applications.length, "application")}`}
      filters={<div className="list-filters">
        <Search aria-label="Search applications" placeholder="Search by name or hostname..." value={query} onChange={(event) => setQuery(event.target.value)} />
        <StatusFilter label="Filter application status" value={status} onChange={setStatus} />
        {filtering ? <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setStatus("all"); }}>Clear filters</Button> : null}
      </div>}>
      <TableHeader><TableRow>
        <TableHead scope="col">Name</TableHead>
        <TableHead scope="col">Status</TableHead>
        <TableHead scope="col" className="wide-only">Hostname</TableHead>
        <TableHead scope="col" className="wide-only">Source</TableHead>
        <TableHead scope="col" className="wide-only">CPU</TableHead>
        <TableHead scope="col" className="wide-only">Memory</TableHead>
      </TableRow></TableHeader>
      <TableBody>
        {!visible.length ? <TableRow><TableCell colSpan={6}>
          No applications match your filters.
        </TableCell></TableRow> : visible.map((app) => {
          const definition = describeDefinition(definitionOf(app));
          const samples = seriesFor(metrics, app.id);
          const unmeasured = app.runtime?.kind === "native" && capabilities?.metrics === false;
          return <TableRow key={app.id} className="row-link" onClick={(event) => {
            if ((event.target as Element).closest("a, button")) return;
            onOpen(app.id);
          }}>
            <TableCell><div className="stack-xs">
              <a className="fg" href={`/console/#app-${app.id}`} onClick={(event) => {
                if (!plainClick(event)) return;
                event.preventDefault();
                onOpen(app.id);
              }}><strong>{app.name}</strong></a>
              <span className="muted t-label mono narrow-only">{app.hostname || "Unpublished"}</span>
            </div></TableCell>
            <TableCell><StatusBadge tone={statusTone(app.status)}>{statusLabel(app.status)}</StatusBadge></TableCell>
            <TableCell className="wide-only mono">
              {app.hostname || <span className="muted">Unpublished</span>}
              {app.aliases?.length ? <span className="t-metadata muted"> +{app.aliases.length}</span> : null}
            </TableCell>
            <TableCell className="wide-only" title={sourceOf(app)}>
              <span className="app-cell"><Icon name={definition.icon} size="md" />{definition.label}</span>
            </TableCell>
            <Usage className="wide-only" samples={samples} unmeasured={unmeasured}
              value={(sample) => sample.cpu_percent} render={(sample) => `${sample.cpu_percent.toFixed(1)}%`} />
            <Usage className="wide-only" samples={samples} unmeasured={unmeasured}
              value={(sample) => sample.memory_bytes} render={(sample) => formatBytes(sample.memory_bytes)} />
          </TableRow>;
        })}
      </TableBody>
    </ListPage>
  );
}
