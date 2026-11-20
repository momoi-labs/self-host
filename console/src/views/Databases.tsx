import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Button, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
  ListPage, Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, StatusBadge,
  TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { RestoreDialog } from "../components/RestoreDialog.js";
import { useToast } from "../components/Toasts.js";
import { formatAgo } from "../lib/format.js";
import { backupSummaryQuery } from "../lib/queries.js";
import { statusTone } from "../lib/status.js";
import type { App } from "../lib/types.js";

export function Databases({ apps, ready, reload, onCreate, onOpen }: {
  apps: App[];
  ready: boolean;
  reload: () => Promise<App[]>;
  onCreate: () => void;
  onOpen: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [restoring, setRestoring] = useState<App | null>(null);
  const notify = useToast();
  const { data: backups = [] } = useQuery({ ...backupSummaryQuery, refetchInterval: 15_000 });
  const databases = apps.filter((app) => app.managed_postgres);
  const visible = databases.filter((database) =>
    database.name.toLowerCase().includes(query.trim().toLowerCase()) &&
    (status === "all" || database.status === status),
  );
  const filtering = query.trim() !== "" || status !== "all";
  const clear = () => { setQuery(""); setStatus("all"); };
  // A server can be restored once its repository holds a backup.
  const sources = databases.filter((database) => backups.some((item) => item.database_application_id === database.id && item.last));

  return <>
    <ListPage title="Databases" action={{ label: "New database", icon: <Icon name="plus" />, onClick: onCreate }}
      state={!ready ? "loading" : databases.length ? "ready" : "empty"}
      empty={{ title: "No databases yet", text: "Create a PostgreSQL server for your Applications to connect to." }}
      count={`${visible.length} of ${databases.length} databases`}
      filters={<div className="list-filters">
        <Search aria-label="Search databases" placeholder="Search databases..." value={query} onChange={(event) => setQuery(event.target.value)} />
        <Select value={status} onValueChange={setStatus}>
          <SelectTrigger aria-label="Filter database status"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All statuses</SelectItem>
            <SelectItem value="running">Running</SelectItem>
            <SelectItem value="stopped">Stopped</SelectItem>
            <SelectItem value="pending">Pending</SelectItem>
            <SelectItem value="failed">Failed</SelectItem>
          </SelectContent>
        </Select>
        {filtering ? <Button size="sm" variant="ghost" onClick={clear}>Clear filters</Button> : null}
      </div>}>
      <TableHeader><TableRow>
        <TableHead scope="col">Database</TableHead>
        <TableHead scope="col">Status</TableHead>
        <TableHead scope="col">Last backup</TableHead>
        <TableHead scope="col" className="col-tight">Actions</TableHead>
      </TableRow></TableHeader>
      <TableBody>
        {!visible.length ? <TableRow><TableCell colSpan={4}>
          No databases match your filters.
        </TableCell></TableRow> : visible.map((database) => <TableRow key={database.id}>
          <TableCell><div className="stack-xs">
            <a className="fg" href={`/console/#app-${database.id}`} onClick={(event) => {
              if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
              event.preventDefault();
              onOpen(database.id);
            }}><strong>{database.name}</strong></a>
            <span className="muted">PostgreSQL {database.managed_postgres?.major}</span>
          </div></TableCell>
          <TableCell><StatusBadge tone={statusTone(database.status)}>{database.status[0].toUpperCase() + database.status.slice(1)}</StatusBadge></TableCell>
          <TableCell>{(() => {
            const row = backups.find((item) => item.database_application_id === database.id);
            if (!row?.enabled && !row?.last) return <span className="muted">Off</span>;
            return <span className="row" title={row.problem ?? (row.last ? new Date(row.last.finished_at).toLocaleString() : undefined)}>
              {row.problem ? <StatusBadge tone="danger">Problem</StatusBadge> : null}
              <span className="muted">{row.last ? formatAgo(row.last.finished_at) : "None yet"}</span>
            </span>;
          })()}</TableCell>
          <TableCell className="col-tight"><DropdownMenu>
            <DropdownMenuTrigger asChild><Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for ${database.name}`}><Icon name="more" /></Button></DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => onOpen(database.id)}>Open database</DropdownMenuItem>
              {sources.includes(database) ? <DropdownMenuItem onSelect={() => setRestoring(database)}>Restore from backup</DropdownMenuItem> : null}
            </DropdownMenuContent>
          </DropdownMenu></TableCell>
        </TableRow>)}
      </TableBody>
    </ListPage>
    <RestoreDialog open={restoring !== null} sources={restoring ? [restoring] : []} onClose={() => setRestoring(null)}
      onRestoring={(id, name, source) => { notify("success", `Creating ${name} from the backups of ${source}`); void reload().then(() => onOpen(id)); }} />
  </>;
}
