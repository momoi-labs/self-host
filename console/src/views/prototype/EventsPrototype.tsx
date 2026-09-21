/**
 * PROTOTYPE. Six renderings of the Events page, switchable via `?variant=`
 * on the existing `#events` route. Two questions:
 *
 *   1. How do we page a long history without losing the reader?
 *   2. How does a row say "queued / running / done / failed" without a
 *      bare coloured dot?
 *
 * Real events come from the API. When the history is short (< 30) a
 * generated fixture stands in so every status and a few pages exist to
 * judge. `?fixture=1` forces it, `?fixture=0` disables it.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";
import {
  Badge, Button, Card, CardContent, CardHeader, Disclosure, Dot,
  PageHeader, PageHeaderDescription, PageHeaderTitle,
  Pagination, PaginationEllipsis, PaginationNext, PaginationPage, PaginationPrevious,
  Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner,
  Stat, StatLabel, StatValue, StepBar,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
  Tabs, TabsList, TabsTrigger,
} from "@momoi-labs/kiso-react";

import { Failure } from "../../components/Failure.js";
import { Icon } from "../../components/Icon.js";
import { PrototypeSwitcher, useVariant } from "../../components/PrototypeSwitcher.js";
import { eventActionLabel, eventSubjectHref, eventUpdatedAt, filterEvents } from "../../lib/platformEvents.js";
import type { PlatformEvent } from "../../lib/platformEvents.js";
import "./events-prototype.css";

/* ------------------------------------------------------------------ */
/* Shared vocabulary                                                   */
/* ------------------------------------------------------------------ */

type Status = PlatformEvent["status"];

/**
 * Running is *info* (blue), not success: the current page paints running and
 * completed the same green, which is half the confusion. Pending is neutral.
 */
const STATUS = {
  pending: { word: "Queued", short: "Waiting", tone: "neutral", hint: "Waiting for its turn" },
  running: { word: "Running", short: "Running", tone: "info", hint: "Being worked on right now" },
  completed: { word: "Done", short: "Done", tone: "success", hint: "Finished without error" },
  failed: { word: "Failed", short: "Failed", tone: "danger", hint: "Stopped with an error" },
} as const satisfies Record<Status, { word: string; short: string; tone: "neutral" | "info" | "success" | "danger"; hint: string }>;

const KIND: Record<PlatformEvent["subject"]["kind"], string> = {
  application: "Application", "virtual-machine": "Virtual machine", "custom-image": "Custom image", "api-key": "API key", "dns-record": "DNS record",
};

const day = new Intl.DateTimeFormat("en-GB", { day: "2-digit", month: "short", year: "numeric" });
const clock = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

function When({ value, mode = "both" }: { value: string; mode?: "both" | "time" | "relative" }) {
  const at = new Date(value);
  if (mode === "time") return <time dateTime={value} className="mono">{clock.format(at)}</time>;
  if (mode === "relative") return <time dateTime={value} title={`${day.format(at)} ${clock.format(at)}`}>{relative(at)}</time>;
  return <time dateTime={value}>{day.format(at)}<span className="events-secondary mono">{clock.format(at)}</span></time>;
}

function relative(at: Date): string {
  const s = Math.max(0, Math.round((Date.now() - at.getTime()) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

function durationOf(event: PlatformEvent): string | null {
  if (!event.startedAt) return null;
  const end = event.finishedAt ? Date.parse(event.finishedAt) : Date.now();
  const s = Math.max(0, Math.round((end - Date.parse(event.startedAt)) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

/** A glyph that means something without colour: check, cross, spinner, clock. */
function StatusGlyph({ status }: { status: Status }) {
  const tone = STATUS[status].tone;
  if (status === "running") return <Spinner size="sm" label="Running" className="proto-glyph" />;
  return <span className={`proto-glyph proto-glyph-${tone}`} role="img" aria-label={STATUS[status].word}>
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {status === "completed" ? <path d="M3 8.5l3.5 3.5L13 5" />
        : status === "failed" ? <><circle cx="8" cy="8" r="6.25" /><path d="M5.5 5.5l5 5M10.5 5.5l-5 5" /></>
        : <><circle cx="8" cy="8" r="6.25" /><path d="M8 4.5V8l2.5 1.5" /></>}
    </svg>
  </span>;
}

/** Badge with a word: the cheapest fix, and a baseline for the others. */
function StatusWord({ status }: { status: Status }) {
  return <Badge variant={STATUS[status].tone} className="proto-word"><Dot pulse={status === "running" || undefined} />{STATUS[status].word}</Badge>;
}

function useSubjectLink(onOpenSubject: (subject: PlatformEvent["subject"]) => void) {
  return (subject: PlatformEvent["subject"], className = "events-subject") => {
    const href = eventSubjectHref(subject);
    const label = subject.service ?? subject.name;
    if (!href) return <span title="This resource is no longer available">{label}</span>;
    return <a className={className} href={href} onClick={(event: MouseEvent<HTMLAnchorElement>) => {
      event.stopPropagation();
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      onOpenSubject(subject);
    }}>{label}</a>;
  };
}

/** Page numbers with an ellipsis either side of the current one. */
function Pager({ page, pages, onPage }: { page: number; pages: number; onPage: (page: number) => void }) {
  if (pages <= 1) return null;
  const items: (number | "…")[] = [];
  for (let n = 1; n <= pages; n++) {
    if (n === 1 || n === pages || Math.abs(n - page) <= 1) items.push(n);
    else if (items[items.length - 1] !== "…") items.push("…");
  }
  return <Pagination aria-label="Event pages">
    <PaginationPrevious disabled={page === 1} onClick={() => onPage(page - 1)} />
    {items.map((item, index) => item === "…" ? <PaginationEllipsis key={`e${index}`} />
      : <PaginationPage key={item} active={item === page} onClick={() => onPage(item)}>{item}</PaginationPage>)}
    <PaginationNext disabled={page === pages} onClick={() => onPage(page + 1)} />
  </Pagination>;
}

function usePage(total: number, size: number, resetKey: unknown) {
  const [page, setPage] = useState(1);
  useEffect(() => setPage(1), [resetKey, size]);
  const pages = Math.max(1, Math.ceil(total / size));
  const current = Math.min(page, pages);
  return { page: current, pages, setPage, from: (current - 1) * size, to: Math.min(current * size, total) };
}

/* ------------------------------------------------------------------ */
/* Fixture                                                             */
/* ------------------------------------------------------------------ */

function fixture(now = Date.now()): PlatformEvent[] {
  const subjects: PlatformEvent["subject"][] = [
    { kind: "virtual-machine", id: "vm-a", name: "a" },
    { kind: "virtual-machine", id: "vm-teste", name: "teste" },
    { kind: "virtual-machine", id: "vm-t1", name: "t1", available: false },
    { kind: "application", id: "app-blog", name: "blog", service: "blog" },
    { kind: "application", id: "app-wiki", name: "wiki", service: "wiki-db" },
    { kind: "application", id: "app-fooo", name: "fooo", available: false },
    { kind: "custom-image", id: "img-node", name: "node-22" },
    { kind: "api-key", id: "key-ci", name: "ci-runner" },
    { kind: "dns-record", id: "rec-nas", name: "nas" },
  ];
  const actions: PlatformEvent["action"][] = ["create", "start", "stop", "restart", "configure", "delete"];
  const out: PlatformEvent[] = [];
  let t = now;
  let seed = 7;
  const rand = () => (seed = (seed * 9301 + 49297) % 233280) / 233280;
  for (let i = 0; i < 92; i++) {
    t -= Math.floor(rand() * 5400_000) + 15_000;
    const subject = subjects[Math.floor(rand() * subjects.length)]!;
    const action = actions[Math.floor(rand() * actions.length)]!;
    const roll = rand();
    const status: Status = i === 0 ? "pending" : i === 1 ? "running" : i === 2 ? "pending" : i === 4 ? "running" : roll < 0.14 ? "failed" : "completed";
    const started = status === "pending" ? null : new Date(t - Math.floor(rand() * 20_000)).toISOString();
    const finished = status === "completed" || status === "failed" ? new Date(t).toISOString() : null;
    out.push({
      id: `fx-${i.toString().padStart(3, "0")}`,
      action, status,
      occurredAt: new Date(t - 30_000).toISOString(),
      startedAt: started, finishedAt: finished, updatedAt: new Date(t).toISOString(),
      apiName: rand() < 0.3 ? "ci-runner" : null,
      description: `${eventActionLabel({ action, subject } as PlatformEvent)} ${subject.name}.`,
      error: status === "failed" ? { error: action === "delete" ? "The machine is still running; stop it first." : action === "create" ? "cloud-init exited with status 1 at step tools" : "The Host could not reach the guest agent within 60s.", caused_by: ["ssh: connect to host 10.0.1.14 port 22: Connection timed out"] } : null,
      subject,
    });
  }
  return out;
}

/* ------------------------------------------------------------------ */
/* Details aside (shared, not the question)                            */
/* ------------------------------------------------------------------ */

function Details({ event, subjectLink, onClose }: { event: PlatformEvent; subjectLink: ReturnType<typeof useSubjectLink>; onClose: () => void }) {
  return <aside className="events-details" onKeyDown={key => { if (key.key === "Escape") onClose(); }}>
    <Card>
      <CardHeader>
        <div className="events-details-top"><span className="t-caps muted">Event details</span><Button size="sm" variant="ghost" aria-label="Close event details" onClick={onClose}><Icon name="x" /></Button></div>
        <h2 className="section-title events-title"><StatusGlyph status={event.status} />{eventActionLabel(event)}</h2>
        <StatusWord status={event.status} />
      </CardHeader>
      <CardContent>
        <p className="events-description">{event.description}</p>
        {event.error ? <div className="events-failure"><Failure failure={event.error} /></div> : null}
        <dl className="events-properties">
          <dt>Affected</dt><dd>{subjectLink(event.subject)}</dd>
          <dt>Resource</dt><dd>{KIND[event.subject.kind]}</dd>
          <dt>Actor</dt><dd className="mono">{event.apiName?.trim() || "system"}</dd>
          <dt>Duration</dt><dd>{durationOf(event) ?? "Not started"}</dd>
          <dt>Last updated</dt><dd><When value={eventUpdatedAt(event)} /></dd>
          <dt>Event ID</dt><dd className="mono">{event.id}</dd>
        </dl>
      </CardContent>
    </Card>
  </aside>;
}

type VariantProps = {
  events: PlatformEvent[];
  subjectLink: ReturnType<typeof useSubjectLink>;
  selected: PlatformEvent | null;
  onSelect: (event: PlatformEvent | null) => void;
};

/* ------------------------------------------------------------------ */
/* A. Status column + numbered pages                                   */
/* ------------------------------------------------------------------ */

function VariantA({ events, subjectLink, selected, onSelect }: VariantProps) {
  const SIZE = 25;
  const { page, pages, setPage, from, to } = usePage(events.length, SIZE, events);
  const rows = events.slice(from, to);
  const scroller = useRef<HTMLDivElement | null>(null);
  useEffect(() => { scroller.current?.scrollTo({ top: 0 }); }, [page]);
  return <div className="table-wrap proto-a">
    <div className="table-scroll" ref={scroller}>
      <Table aria-label="Platform events">
        <TableHeader><TableRow>
          <TableHead className="proto-a-status"><span className="sr-only">Status</span></TableHead><TableHead>Action</TableHead><TableHead>Affected</TableHead><TableHead>Actor</TableHead><TableHead>Last updated</TableHead>
        </TableRow></TableHeader>
        <TableBody>{rows.map(event => <TableRow key={event.id} className={`events-row proto-a-${event.status}`} data-state={selected?.id === event.id ? "selected" : undefined} onClick={() => onSelect(event)}>
          <TableCell className="proto-a-status"><span title={`${STATUS[event.status].word}: ${STATUS[event.status].hint}`}><StatusGlyph status={event.status} /></span></TableCell>
          <TableCell>{eventActionLabel(event)}{event.status === "failed" && event.error ? <span className="proto-error-line">{event.error.error}</span> : null}</TableCell>
          <TableCell>{subjectLink(event.subject)}<span className="events-secondary">{KIND[event.subject.kind]}</span></TableCell>
          <TableCell><span className={`mono${event.apiName ? "" : " muted"}`}>{event.apiName?.trim() || "system"}</span></TableCell>
          <TableCell><When value={eventUpdatedAt(event)} /></TableCell>
        </TableRow>)}</TableBody>
      </Table>
    </div>
    <div className="proto-foot">
      <span role="status">{events.length ? `${from + 1}–${to} of ${events.length}` : "0 events"}</span>
      <Pager page={page} pages={pages} onPage={setPage} />
    </div>
  </div>;
}

/* ------------------------------------------------------------------ */
/* B. Day timeline + load more                                         */
/* ------------------------------------------------------------------ */

function dayKey(iso: string): string {
  const at = new Date(iso); const today = new Date();
  const diff = Math.floor((new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime() - new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime()) / 86400000);
  return diff === 0 ? "Today" : diff === 1 ? "Yesterday" : day.format(at);
}

function VariantB({ events, subjectLink, selected, onSelect }: VariantProps) {
  const STEP = 25;
  const [shown, setShown] = useState(STEP);
  useEffect(() => setShown(STEP), [events]);
  const visible = events.slice(0, shown);
  const groups: { label: string; items: PlatformEvent[] }[] = [];
  for (const event of visible) {
    const label = dayKey(eventUpdatedAt(event));
    const last = groups[groups.length - 1];
    if (last?.label === label) last.items.push(event); else groups.push({ label, items: [event] });
  }
  return <div className="proto-timeline">
    {groups.map(group => <section key={group.label} className="proto-day">
      <h3 className="t-caps muted proto-day-title">{group.label} <span>{group.items.length}</span></h3>
      <ol>{group.items.map(event => <li key={event.id} className={`proto-tl-row proto-tl-${event.status}${selected?.id === event.id ? " is-selected" : ""}`}>
        <button type="button" className="proto-tl-main" onClick={() => onSelect(event)}>
          <StatusGlyph status={event.status} />
          <span className="proto-tl-text">
            <span>{eventActionLabel(event)} <span className="muted">·</span> {subjectLink(event.subject, "proto-inline-link")}</span>
            <span className="events-secondary">{STATUS[event.status].word}{event.status === "failed" && event.error ? `: ${event.error.error}` : durationOf(event) ? ` ${event.status === "running" ? "for" : "in"} ${durationOf(event)}` : ""} · {event.apiName?.trim() || "system"}</span>
          </span>
        </button>
        <When value={eventUpdatedAt(event)} mode="time" />
      </li>)}</ol>
    </section>)}
    <div className="proto-foot proto-foot-center">
      <span role="status">{Math.min(shown, events.length)} of {events.length} shown</span>
      {shown < events.length ? <Button size="sm" onClick={() => setShown(shown + STEP)}>Show {Math.min(STEP, events.length - shown)} older</Button> : null}
    </div>
  </div>;
}

/* ------------------------------------------------------------------ */
/* C. Attention first: counts, tabs, scroll region                     */
/* ------------------------------------------------------------------ */

function VariantC({ events, subjectLink, selected, onSelect }: VariantProps) {
  const [tab, setTab] = useState("attention");
  const failed = events.filter(one => one.status === "failed");
  const active = events.filter(one => one.status === "running" || one.status === "pending");
  const list = tab === "attention" ? failed : tab === "active" ? active : events;
  const STEP = 30;
  const [shown, setShown] = useState(STEP);
  useEffect(() => setShown(STEP), [list.length, tab]);
  const sentinel = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const node = sentinel.current; if (!node) return;
    const io = new IntersectionObserver(entries => { if (entries.some(entry => entry.isIntersecting)) setShown(value => value + STEP); });
    io.observe(node); return () => io.disconnect();
  }, [list.length]);
  return <div className="proto-attention">
    <div className="proto-stats">
      <button type="button" className={`proto-stat${tab === "attention" ? " is-on" : ""}`} onClick={() => setTab("attention")}>
        <Stat><StatLabel>Needs attention</StatLabel><StatValue className={failed.length ? "proto-danger" : ""}>{failed.length}</StatValue></Stat>
      </button>
      <button type="button" className={`proto-stat${tab === "active" ? " is-on" : ""}`} onClick={() => setTab("active")}>
        <Stat><StatLabel>In progress</StatLabel><StatValue>{active.length}{active.length ? <Spinner size="sm" className="proto-stat-spin" label="" /> : null}</StatValue></Stat>
      </button>
      <button type="button" className={`proto-stat${tab === "all" ? " is-on" : ""}`} onClick={() => setTab("all")}>
        <Stat><StatLabel>All events</StatLabel><StatValue>{events.length}</StatValue></Stat>
      </button>
    </div>
    <Tabs value={tab} onValueChange={setTab}>
      <TabsList aria-label="Which events">
        <TabsTrigger value="attention">Needs attention</TabsTrigger>
        <TabsTrigger value="active">In progress</TabsTrigger>
        <TabsTrigger value="all">Everything</TabsTrigger>
      </TabsList>
    </Tabs>
    <div className="proto-scroll" role="region" aria-label="Events" tabIndex={0}>
      <table className="proto-scroll-table">
        <thead><tr><th>Status</th><th>What</th><th>Affected</th><th>When</th></tr></thead>
        <tbody>{list.slice(0, shown).map(event => <tr key={event.id} className={`events-row${selected?.id === event.id ? " is-selected" : ""}`} onClick={() => onSelect(event)}>
          <td><StatusGlyph status={event.status} /> <span className="proto-word-plain">{STATUS[event.status].word}</span></td>
          <td>{eventActionLabel(event)}{event.status === "failed" && event.error ? <span className="proto-error-line">{event.error.error}</span> : null}</td>
          <td>{subjectLink(event.subject)}<span className="events-secondary">{KIND[event.subject.kind]}</span></td>
          <td><When value={eventUpdatedAt(event)} mode="relative" /></td>
        </tr>)}</tbody>
      </table>
      {!list.length ? <p className="proto-empty">{tab === "attention" ? "Nothing failed. Good." : tab === "active" ? "Nothing is running or queued." : "No events."}</p> : null}
      {shown < list.length ? <div ref={sentinel} className="proto-sentinel"><Spinner size="sm" label="Loading older events" /></div> : null}
    </div>
    <p className="table-footer"><span role="status">{Math.min(shown, list.length)} of {list.length} in this view · scroll for older</span></p>
  </div>;
}

/* ------------------------------------------------------------------ */
/* D. Grouped by resource                                              */
/* ------------------------------------------------------------------ */

function VariantD({ events, subjectLink, selected, onSelect }: VariantProps) {
  const groups = useMemo(() => {
    const map = new Map<string, { subject: PlatformEvent["subject"]; items: PlatformEvent[] }>();
    for (const event of events) {
      const key = `${event.subject.kind}:${event.subject.id}`;
      const group = map.get(key) ?? { subject: event.subject, items: [] };
      group.items.push(event); map.set(key, group);
    }
    return [...map.values()];
  }, [events]);
  const SIZE = 6;
  const { page, pages, setPage, from, to } = usePage(groups.length, SIZE, events);
  const headline = (items: PlatformEvent[]) => {
    const latest = items[0]!;
    const verb = eventActionLabel(latest).split(" ")[0]!.toLowerCase();
    if (latest.status === "running") return <><Spinner size="sm" label="" /> {verb === "delete" ? "Deleting" : verb === "create" ? "Creating" : `${verb.replace(/e$/, "")}ing`}…</>;
    if (latest.status === "pending") return <><StatusGlyph status="pending" /> Waiting to {verb}</>;
    if (latest.status === "failed") return <><StatusGlyph status="failed" /> Last {verb} failed</>;
    return <><StatusGlyph status="completed" /> Idle · last {verb} <When value={eventUpdatedAt(latest)} mode="relative" /></>;
  };
  return <div className="proto-groups">
    {groups.slice(from, to).map(group => {
      const failed = group.items.filter(one => one.status === "failed").length;
      return <Card key={`${group.subject.kind}:${group.subject.id}`} className={`proto-group proto-group-${group.items[0]!.status}`}>
        <CardHeader className="proto-group-head">
          <div>
            <h3 className="section-title">{subjectLink(group.subject)} <span className="events-secondary">{KIND[group.subject.kind]}</span></h3>
            <p className="proto-group-state">{headline(group.items)}</p>
          </div>
          <div className="proto-group-counts">
            <span>{group.items.length} events</span>{failed ? <Badge variant="danger">{failed} failed</Badge> : null}
          </div>
        </CardHeader>
        <CardContent>
          <ol className="proto-group-list">{group.items.slice(0, 3).map(event => <li key={event.id} className={selected?.id === event.id ? "is-selected" : ""}>
            <button type="button" onClick={() => onSelect(event)}><StatusGlyph status={event.status} /> {eventActionLabel(event).split(" ")[0]} <span className="muted">· {STATUS[event.status].word}{event.error ? `: ${event.error.error}` : ""}</span></button>
            <When value={eventUpdatedAt(event)} mode="relative" />
          </li>)}</ol>
          {group.items.length > 3 ? <Disclosure summary={`Show ${group.items.length - 3} older`}>
            <ol className="proto-group-list">{group.items.slice(3).map(event => <li key={event.id} className={selected?.id === event.id ? "is-selected" : ""}>
              <button type="button" onClick={() => onSelect(event)}><StatusGlyph status={event.status} /> {eventActionLabel(event).split(" ")[0]} <span className="muted">· {STATUS[event.status].word}</span></button>
              <When value={eventUpdatedAt(event)} mode="relative" />
            </li>)}</ol>
          </Disclosure> : null}
        </CardContent>
      </Card>;
    })}
    <div className="proto-foot">
      <span role="status">{groups.length ? `Resources ${from + 1}–${to} of ${groups.length}` : "No resources"} · sorted by latest activity</span>
      <Pager page={page} pages={pages} onPage={setPage} />
    </div>
  </div>;
}

/* ------------------------------------------------------------------ */
/* E. Dense ledger                                                     */
/* ------------------------------------------------------------------ */

function VariantE({ events, subjectLink, selected, onSelect }: VariantProps) {
  const [size, setSize] = useState(50);
  const { page, pages, setPage, from, to } = usePage(events.length, size, events);
  return <div className="proto-ledger-wrap">
    <div className="proto-ledger" role="table" aria-label="Platform events">
      {events.slice(from, to).map(event => <div key={event.id} role="row" className={`proto-ledger-row proto-ledger-${event.status}${selected?.id === event.id ? " is-selected" : ""}`} onClick={() => onSelect(event)}>
        <span role="cell" className="mono muted proto-ledger-time"><When value={eventUpdatedAt(event)} mode="time" /></span>
        <span role="cell" className="proto-ledger-status" title={`${STATUS[event.status].word}: ${STATUS[event.status].hint}`}><StatusGlyph status={event.status} /></span>
        <span role="cell" className="proto-ledger-what">{eventActionLabel(event)}</span>
        <span role="cell" className="proto-ledger-who">{subjectLink(event.subject, "proto-inline-link")} <span className="muted">{KIND[event.subject.kind].toLowerCase()}</span></span>
        <span role="cell" className="mono muted proto-ledger-actor">{event.apiName?.trim() || "system"}</span>
        <span role="cell" className="mono muted proto-ledger-dur">{durationOf(event) ?? ""}</span>
        {event.status === "failed" && event.error ? <span role="cell" className="proto-ledger-err">↳ {event.error.error}</span> : null}
      </div>)}
    </div>
    <div className="proto-foot">
      <span role="status">{events.length ? `${from + 1}–${to} of ${events.length}` : "0 events"} · {dayKey(events[from] ? eventUpdatedAt(events[from]!) : new Date().toISOString())}</span>
      <div className="proto-foot-right">
        <Select value={String(size)} onValueChange={value => setSize(Number(value))}>
          <SelectTrigger aria-label="Rows per page"><SelectValue /></SelectTrigger>
          <SelectContent>{[25, 50, 100].map(n => <SelectItem key={n} value={String(n)}>{n} per page</SelectItem>)}</SelectContent>
        </Select>
        <Button size="sm" variant="ghost" disabled={page === 1} onClick={() => setPage(page - 1)}>← Newer</Button>
        <span className="mono muted">{page}/{pages}</span>
        <Button size="sm" variant="ghost" disabled={page === pages} onClick={() => setPage(page + 1)}>Older →</Button>
      </div>
    </div>
  </div>;
}

/* ------------------------------------------------------------------ */
/* F. Lifecycle bar per row                                            */
/* ------------------------------------------------------------------ */

function lifecycle(event: PlatformEvent) {
  const s = event.status;
  return [
    { key: "queued", label: "Queued", state: "done" as const },
    { key: "running", label: "Running", state: s === "pending" ? "pending" as const : s === "running" ? "running" as const : "done" as const },
    { key: "end", label: s === "failed" ? "Failed" : "Done", state: s === "completed" ? "done" as const : s === "failed" ? "failed" as const : "pending" as const },
  ];
}

function VariantF({ events, subjectLink, selected, onSelect }: VariantProps) {
  const [size, setSize] = useState(15);
  const { page, pages, setPage, from, to } = usePage(events.length, size, events);
  return <div className="proto-lifecycle">
    <ol>{events.slice(from, to).map(event => {
      const d = durationOf(event);
      const caption = event.status === "completed" ? `Completed${d ? ` in ${d}` : ""}` : event.status === "running" ? `Running${d ? ` for ${d}` : ""}` : event.status === "pending" ? "Queued, waiting for its turn" : `Failed${event.error ? `: ${event.error.error}` : ""}`;
      return <li key={event.id} className={`proto-lc-row proto-lc-${event.status}${selected?.id === event.id ? " is-selected" : ""}`} onClick={() => onSelect(event)}>
        <div className="proto-lc-text">
          <strong>{eventActionLabel(event)}</strong> <span className="muted">on</span> {subjectLink(event.subject, "proto-inline-link")}
          <span className="events-secondary">{caption} · {event.apiName?.trim() || "system"}</span>
        </div>
        <div className="proto-lc-bar"><StepBar label={`${eventActionLabel(event)} progress`} steps={lifecycle(event)} /></div>
        <When value={eventUpdatedAt(event)} mode="relative" />
      </li>;
    })}</ol>
    <div className="proto-foot">
      <span role="status">{events.length ? `${from + 1}–${to} of ${events.length}` : "0 events"}</span>
      <div className="proto-foot-right">
        <Select value={String(size)} onValueChange={value => setSize(Number(value))}>
          <SelectTrigger aria-label="Rows per page"><SelectValue /></SelectTrigger>
          <SelectContent>{[15, 30, 60].map(n => <SelectItem key={n} value={String(n)}>{n} per page</SelectItem>)}</SelectContent>
        </Select>
        <Pager page={page} pages={pages} onPage={setPage} />
      </div>
    </div>
  </div>;
}

/* ------------------------------------------------------------------ */
/* Route                                                               */
/* ------------------------------------------------------------------ */

const VARIANTS = [
  { key: "A", name: "Status column + numbered pages", render: VariantA },
  { key: "B", name: "Day timeline + show older", render: VariantB },
  { key: "C", name: "Attention first + scroll region", render: VariantC },
  { key: "D", name: "Grouped by resource", render: VariantD },
  { key: "E", name: "Dense ledger + page size", render: VariantE },
  { key: "F", name: "Lifecycle bar per row", render: VariantF },
] as const;

export function EventsPrototype({ events: real, onOpenSubject }: { events: readonly PlatformEvent[]; onOpenSubject: (subject: PlatformEvent["subject"]) => void }) {
  const variant = useVariant("A");
  const flag = new URLSearchParams(location.search).get("fixture");
  const useFixture = flag === "1" || (flag !== "0" && real.length < 30);
  const fake = useMemo(() => fixture(), []);
  const all = useFixture ? fake : real;
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const events = useMemo(() => filterEvents(all, query, status), [all, query, status]);
  const selected = events.find(one => one.id === selectedId) ?? null;
  const subjectLink = useSubjectLink(onOpenSubject);
  const Render = (VARIANTS.find(one => one.key === variant) ?? VARIANTS[0]).render;
  const filtered = query !== "" || status !== "all";

  return <div className={`events-page proto-page${variant === "A" ? " proto-fit" : ""}`}>
    <PageHeader>
      <PageHeaderTitle>Events</PageHeaderTitle>
      <PageHeaderDescription>Activity across your services and virtual machines.</PageHeaderDescription>
    </PageHeader>
    <div className={`events-layout${selected ? " events-layout-open" : ""}`}>
      <div className="events-list">
        <div className="list-filters">
          <Search aria-label="Filter by service" placeholder="Search services or virtual machines..." value={query} onChange={event => { setQuery(event.target.value); setSelectedId(null); }} />
          <Select value={status} onValueChange={value => { setStatus(value); setSelectedId(null); }}>
            <SelectTrigger aria-label="Filter event status"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All statuses</SelectItem>
              {(Object.keys(STATUS) as Status[]).map(key => <SelectItem key={key} value={key}>{STATUS[key].word}</SelectItem>)}
            </SelectContent>
          </Select>
          {filtered ? <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setStatus("all"); setSelectedId(null); }}>Clear filters</Button> : null}
        </div>
        <Render events={events} subjectLink={subjectLink} selected={selected} onSelect={event => setSelectedId(event?.id ?? null)} />
      </div>
      {selected ? <Details event={selected} subjectLink={subjectLink} onClose={() => setSelectedId(null)} /> : null}
    </div>
    <PrototypeSwitcher variants={VARIANTS} current={variant} note={useFixture ? `fixture, ${all.length} events` : `live, ${all.length} events`} />
  </div>;
}
