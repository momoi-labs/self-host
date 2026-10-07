import { useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Button, Card, CardContent, CardHeader,
  Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, EmptyStateDescription, EmptyStateTitle, FormField,
  KV, KVKey, KVValue, PageHeader, PageHeaderDescription, PageHeaderTitle, Search,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { Failure } from "../components/Failure.js";
import { Icon } from "../components/Icon.js";
import { api, asReport, failureOf, readJson } from "../lib/api.js";
import { recordRows, type DnsRecord, type RecordRow } from "../lib/dnsRecords.js";
import { bootstrapStatusQuery } from "../lib/queries.js";
import type { App, Environment, Report } from "../lib/types.js";

type Editor = { mode: "create" } | { mode: "edit" | "delete"; record: RecordRow };

export function Dns({ applications, machines, onOpenApplication, onOpenVirtualMachine }: {
  applications: App[];
  machines: Environment[];
  onOpenApplication: (id: string) => void;
  onOpenVirtualMachine: (id: string) => void;
}) {
  const records = useQuery({
    queryKey: ["dns", "records"],
    queryFn: ({ signal }) => readJson<DnsRecord[]>("/dns/records", signal),
    refetchInterval: 5000,
  });
  const status = useQuery({ ...bootstrapStatusQuery, refetchInterval: 5000 });
  const inventory = records.data ?? null;
  const diagnostics = status.data ?? null;
  const loadFailure = records.error ?? status.error;
  const [query, setQuery] = useState("");
  const [owner, setOwner] = useState("all");
  const [editor, setEditor] = useState<Editor | null>(null);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [description, setDescription] = useState("");
  const [failure, setFailure] = useState<Report | null>(null);
  const [saving, setSaving] = useState(false);

  function refresh() {
    void records.refetch();
    void status.refetch();
  }

  function open(next: Editor) {
    setEditor(next);
    setName(next.mode === "create" ? "" : next.record.name);
    setValue(next.mode === "create" ? "" : next.record.value);
    setDescription(next.mode === "create" ? "" : next.record.description ?? "");
    setFailure(null);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!editor || saving) return;
    setSaving(true);
    setFailure(null);
    try {
      const path = editor.mode === "create" ? "/dns/records"
        : `/dns/records/${encodeURIComponent(editor.record.name)}/${editor.record.type}`;
      const response = await api(path, {
        method: editor.mode === "create" ? "POST" : editor.mode === "edit" ? "PUT" : "DELETE",
        body: editor.mode === "delete" ? undefined : JSON.stringify({
          name: editor.mode === "create" || name !== editor.record.name ? name : undefined,
          ...(editor.mode === "create" ? { type: "A" } : {}),
          value, description,
        }),
      });
      if (!response.ok) throw await failureOf(response);
      setEditor(null);
      refresh();
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSaving(false);
    }
  }

  const rows = recordRows(inventory ?? [], applications, machines);
  const visible = rows.filter(row =>
    (owner === "all" || row.owner === owner)
    && `${row.name} ${row.values.join(" ")} ${row.description ?? ""}`.toLowerCase().includes(query.trim().toLowerCase()));
  const filtering = query !== "" || owner !== "all";
  const action = editor?.mode === "delete" ? "Delete record" : editor?.mode === "edit" ? "Save record" : "Create record";
  const renamable = editor?.mode === "create" || (editor?.mode === "edit" && editor.record.renamable);

  return (
    <>
      <PageHeader actions={<Button size="sm" variant="primary" onClick={() => open({ mode: "create" })}><Icon name="plus" />New record</Button>}>
        <PageHeaderTitle>DNS</PageHeaderTitle>
        <PageHeaderDescription>Names the Platform answers for, and the Records you manage.</PageHeaderDescription>
      </PageHeader>
      {loadFailure ? <Failure failure={loadFailure} actionLabel="Retry" onAction={refresh} /> : null}
      <div className="list-filters">
        <Search aria-label="Search records" placeholder="Search records..." value={query} onChange={event => setQuery(event.target.value)} />
        <Select value={owner} onValueChange={setOwner}>
          <SelectTrigger aria-label="Filter record owner"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All owners</SelectItem>
            <SelectItem value="platform">Platform</SelectItem>
            <SelectItem value="application">Application</SelectItem>
            <SelectItem value="virtual-machine">Virtual machine</SelectItem>
            <SelectItem value="operator">Operator</SelectItem>
          </SelectContent>
        </Select>
        {filtering ? <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setOwner("all"); }}>Clear filters</Button> : null}
      </div>
      {inventory === null ? <p className="muted">Loading records...</p> : rows.length === 0 ? (
        <EmptyState variant="first-run"><EmptyStateTitle>No records</EmptyStateTitle><EmptyStateDescription>Create a Record or check the Zone diagnostics below.</EmptyStateDescription></EmptyState>
      ) : (
        <div className="table-wrap">
          <div className="table-scroll">
            <Table>
              <TableHeader><TableRow>
                <TableHead>Name</TableHead><TableHead>Record Type</TableHead><TableHead>Value</TableHead>
                <TableHead>TTL</TableHead><TableHead>Owner</TableHead><TableHead>Origin</TableHead><TableHead>Actions</TableHead>
              </TableRow></TableHeader>
              <TableBody>
                {visible.length === 0 ? <TableRow><TableCell colSpan={7}>No records match your filters.</TableCell></TableRow> : visible.map(row => (
                  <TableRow key={`${row.name}/${row.type}`}>
                    <TableCell className="mono">{row.name}</TableCell>
                    <TableCell>{row.type}</TableCell>
                    <TableCell className="mono">{row.values.map(address => <div key={address}>{address}</div>)}</TableCell>
                    <TableCell>{row.ttl} s</TableCell>
                    <TableCell>{row.owner === "platform" ? "Platform" : row.owner === "application" ? "Application" : row.owner === "virtual-machine" ? "Virtual machine" : "Operator"}</TableCell>
                    <TableCell>{row.originHref ? <a href={row.originHref} onClick={event => {
                      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                      event.preventDefault();
                      if (row.owner === "application" && row.application_id) onOpenApplication(row.application_id);
                      else if (row.virtual_machine_id) onOpenVirtualMachine(row.virtual_machine_id);
                    }}>{row.origin}</a> : row.origin}</TableCell>
                    <TableCell>{row.editable ? <div className="row">
                      <Button size="sm" variant="ghost" aria-label={`Edit ${row.name}`} onClick={() => open({ mode: "edit", record: row })}>Edit</Button>
                      <Button size="sm" variant="ghost" className="btn-danger-ghost" aria-label={`Delete ${row.name}`} onClick={() => open({ mode: "delete", record: row })}>Delete</Button>
                    </div> : null}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
          <p className="table-footer"><span>{visible.length} of {rows.length} records</span></p>
        </div>
      )}
      <Card>
        <CardHeader><h2 className="t-caps">Zone diagnostics</h2></CardHeader>
        <CardContent>
          {loadFailure && diagnostics ? <p className="muted">Last successful reading. Retrying...</p> : null}
          <KV>
            <KVKey>DNS Suffix</KVKey><KVValue>{diagnostics ? diagnostics.dns_suffix ?? "Unavailable" : "Loading..."}</KVValue>
            <KVKey>Published Host addresses</KVKey><KVValue>{diagnostics ? diagnostics.host_addresses.join(", ") || "None. Check the Host network interface and DNS service." : "Loading..."}</KVValue>
            <KVKey>Upstream forwarders</KVKey><KVValue>{diagnostics ? diagnostics.forwarders.join(", ") || "None" : "Loading..."}</KVValue>
          </KV>
        </CardContent>
      </Card>
      <Dialog open={editor !== null} onOpenChange={isOpen => { if (!isOpen && !saving) setEditor(null); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{editor?.mode === "create" ? "New record" : editor?.mode === "edit" ? "Edit record" : "Delete record"}</DialogTitle>
            <DialogDescription>{editor?.mode === "delete" ? `Delete ${name}? The wildcard will answer for this name again.` : `A Records under ${diagnostics?.dns_suffix ?? "the DNS Suffix"} have a fixed TTL of 60 seconds.`}</DialogDescription>
          </DialogHeader>
          <form onSubmit={submit}>
            <DialogBody>
              {editor?.mode !== "delete" ? <>
                {renamable ? <FormField id="record-name" label="Name" required autoFocus disabled={saving} value={name} onChange={event => setName(event.target.value)} placeholder="nas" /> : null}
                <FormField id="record-value" label="IPv4 address" required autoFocus={!renamable} disabled={saving} value={value} onChange={event => setValue(event.target.value)} placeholder="192.168.1.30" />
                <FormField id="record-description" label="Description" disabled={saving} value={description} onChange={event => setDescription(event.target.value)} />
              </> : null}
              {failure ? <Failure failure={failure} /> : null}
            </DialogBody>
            <DialogFooter>
              <Button size="sm" type="button" disabled={saving} onClick={() => setEditor(null)}>Cancel</Button>
              <Button size="sm" type="submit" variant={editor?.mode === "delete" ? "destructive" : "primary"} disabled={saving}>{saving ? "Saving..." : action}</Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
