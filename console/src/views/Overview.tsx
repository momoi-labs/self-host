import {
  Button,
  Card,
  EmptyState,
  EmptyStateActions,
  EmptyStateDescription,
  EmptyStateIcon,
  EmptyStateTitle,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Search,
  Stat,
  StatLabel,
  StatValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { statusTone } from "../lib/status.js";
import type { App } from "../lib/types.js";

type Row = {
  key: string;
  id: string;
  name: string;
  hostname?: string;
  aliases: number;
  image: string;
  status: string;
  restarts?: number | null;
};

export function Overview({
  apps,
  dnsSuffix,
  query,
  onQuery,
  onOpenApp,
  onDeploy,
}: {
  apps: App[];
  dnsSuffix: string;
  query: string;
  onQuery: (query: string) => void;
  onOpenApp: (id: string) => void;
  onDeploy: () => void;
}) {
  const rows: Row[] = apps.map((app) => ({
    key: app.id,
    id: app.id,
    name: app.name,
    hostname: app.hostname,
    aliases: (app.aliases ?? []).length,
    image: app.image,
    status: app.status,
    restarts: app.restarts,
  }));

  const needle = query.trim().toLowerCase();
  const visible = rows.filter((row) => row.name.toLowerCase().includes(needle));
  const empty = rows.length === 0 && !needle;
  const running = apps.filter((app) => app.status === "running").length;
  const failed = apps.filter((app) => app.status === "failed").length;

  const open = (row: Row) => onOpenApp(row.id);

  return (
    <>
      <PageHeader>
        <PageHeaderTitle>Overview</PageHeaderTitle>
        <PageHeaderDescription>
          {apps.length === 0
            ? `Nothing deployed on ${dnsSuffix} yet.`
            : `${apps.length} ${apps.length === 1 ? "application on" : "applications on"} ${dnsSuffix}`}
        </PageHeaderDescription>
      </PageHeader>

      <div className="dashboard-filters">
        <Search
          aria-label="Search by name"
          placeholder="Search by name…"
          value={query}
          onChange={(event) => onQuery(event.target.value)}
        />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            onQuery("");
          }}
        >
          Clear filters
        </Button>
      </div>

      {empty ? (
        <Card>
          <EmptyState variant="first-run" className="hatch">
            <EmptyStateIcon>
              <Icon name="box" size="lg" />
            </EmptyStateIcon>
            <EmptyStateTitle>No applications yet</EmptyStateTitle>
            <EmptyStateDescription>
              Deploy your first application from a container image or a Compose file and it will be
              reachable on your LAN.
            </EmptyStateDescription>
            <EmptyStateActions>
              <Button size="sm" variant="primary" onClick={onDeploy}>
                <Icon name="plus" />
                Deploy application
              </Button>
            </EmptyStateActions>
          </EmptyState>
        </Card>
      ) : (
        <div className="stack">
          <div className="stat-grid">
            <Card>
              <Stat>
                <StatLabel>Applications</StatLabel>
                <StatValue>{apps.length}</StatValue>
              </Stat>
            </Card>
            <Card>
              <Stat>
                <StatLabel>Running</StatLabel>
                <StatValue>{running}</StatValue>
              </Stat>
            </Card>
            <Card>
              <Stat>
                <StatLabel>Failed</StatLabel>
                <StatValue className={failed ? "danger" : undefined}>{failed}</StatValue>
              </Stat>
            </Card>
          </div>

          <div className="table-wrap">
            <div className="table-scroll">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Name</TableHead>
                    <TableHead scope="col">Hostname</TableHead>
                    <TableHead scope="col">Image</TableHead>
                    <TableHead scope="col">Status</TableHead>
                    <TableHead scope="col" className="num">
                      Restarts
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visible.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={5} className="muted">
                        No applications match your filters.
                      </TableCell>
                    </TableRow>
                  ) : (
                    visible.map((row) => (
                      <TableRow
                        key={row.key}
                        tabIndex={0}
                        role="button"
                        aria-label={`Open ${row.name}`}
                        onClick={() => open(row)}
                        onKeyDown={(event) => {
                          if (event.key !== "Enter" && event.key !== " ") return;
                          event.preventDefault();
                          open(row);
                        }}
                      >
                        <TableCell>{row.name}</TableCell>
                        <TableCell className="mono">
                          {row.hostname}
                          {row.aliases ? (
                            <span className="t-metadata muted"> +{row.aliases}</span>
                          ) : null}
                        </TableCell>
                        <TableCell className="mono">{row.image}</TableCell>
                        <TableCell>
                          <StatusBadge tone={statusTone(row.status)}>{row.status}</StatusBadge>
                        </TableCell>
                        <TableCell className="num">
                          {row.restarts === null || row.restarts === undefined ? (
                            <span className="muted">—</span>
                          ) : (
                            row.restarts
                          )}
                        </TableCell>
                      </TableRow>
                    ))
                  )}
                </TableBody>
              </Table>
            </div>
            <p className="table-footer">
              {visible.length} of {rows.length} applications
            </p>
          </div>
        </div>
      )}
    </>
  );
}
