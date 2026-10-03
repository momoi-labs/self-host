import { useState } from "react";
import {
  Button, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
  EmptyState, EmptyStateActions, EmptyStateTitle,
  PageHeader, PageHeaderTitle, Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Skeleton, StatusBadge,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { Icon } from "../components/Icon.js";
import { statusTone } from "../lib/status.js";
import type { App } from "../lib/types.js";

export function Databases({ apps, ready, onCreate, onOpen }: {
  apps: App[];
  ready: boolean;
  onCreate: () => void;
  onOpen: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const databases = apps.filter((app) => app.managed_postgres);
  const visible = databases.filter((database) =>
    database.name.toLowerCase().includes(query.trim().toLowerCase()) &&
    (status === "all" || database.status === status),
  );
  const filtering = query.trim() !== "" || status !== "all";
  const clear = () => { setQuery(""); setStatus("all"); };
  const create = <Button size="sm" variant="primary" onClick={onCreate}><Icon name="plus" />New database</Button>;

  return <>
    <PageHeader actions={create}><PageHeaderTitle>Databases</PageHeaderTitle></PageHeader>
    <div className="list-filters">
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
    </div>
    <div className="table-wrap" aria-busy={!ready}>
      <div className="table-scroll"><Table aria-label="Databases">
        <TableHeader><TableRow>
          <TableHead scope="col">Database</TableHead>
          <TableHead scope="col">Status</TableHead>
          <TableHead scope="col" className="col-tight">Actions</TableHead>
        </TableRow></TableHeader>
        <TableBody>
          {!ready ? [0, 1, 2].map((row) => <TableRow key={row}>
            <TableCell><div className="stack-xs"><Skeleton /><Skeleton /></div></TableCell>
            <TableCell><Skeleton /></TableCell><TableCell><Skeleton /></TableCell>
          </TableRow>) : !databases.length ? <TableRow><TableCell colSpan={3}>
            <EmptyState variant="first-run">
              <EmptyStateTitle>No databases yet</EmptyStateTitle>
              <EmptyStateActions>{create}</EmptyStateActions>
            </EmptyState>
          </TableCell></TableRow> : !visible.length ? <TableRow><TableCell colSpan={3}>
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
            <TableCell className="col-tight"><DropdownMenu>
              <DropdownMenuTrigger asChild><Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for ${database.name}`}><Icon name="more" /></Button></DropdownMenuTrigger>
              <DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => onOpen(database.id)}>Open database</DropdownMenuItem></DropdownMenuContent>
            </DropdownMenu></TableCell>
          </TableRow>)}
        </TableBody>
      </Table></div>
      <p className="table-footer" role="status"><span>{ready ? `${visible.length} of ${databases.length} databases` : "Loading databases..."}</span></p>
    </div>
  </>;
}
