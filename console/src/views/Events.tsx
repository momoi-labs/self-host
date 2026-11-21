import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import {
  Badge, Button, Card, CardContent, CardHeader,
  EmptyState, EmptyStateDescription, EmptyStateTitle,
  PageHeader, PageHeaderTitle,
  Pagination, PaginationEllipsis, PaginationNext, PaginationPage, PaginationPrevious,
  Search, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Skeleton, Spinner,
  Table, TableFrame, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { Failure } from "../components/Failure.js";
import { Changes } from "../components/Changes.js";
import { Icon } from "../components/Icon.js";
import { eventActionLabel, eventSource, eventStatusLabel, eventSubjectHref, eventSubjectKindLabel, eventUpdatedAt, filterEvents, pageItems } from "../lib/platformEvents.js";
import type { PlatformEvent } from "../lib/platformEvents.js";
import { useEvents } from "../lib/useEvents.js";
import "./events.css";

const PAGE_SIZE = 25;
const statuses: PlatformEvent["status"][] = ["pending", "running", "completed", "failed"];
const dayFormat = new Intl.DateTimeFormat("en-GB", { day: "2-digit", month: "short", year: "numeric" });
const timeFormat = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

function Timestamp({ value, inline = false }: { value: string; inline?: boolean }) {
  const at = new Date(value);
  if (inline) return <time dateTime={value}>{dayFormat.format(at)} {timeFormat.format(at)}</time>;
  return <time dateTime={value}>{dayFormat.format(at)}<span className="events-secondary mono">{timeFormat.format(at)}</span></time>;
}

/**
 * A glyph that says what the task is doing without needing its colour: a
 * clock waits, a spinner works, a check is done, a crossed circle failed.
 */
function StatusGlyph({ status }: { status: PlatformEvent["status"] }) {
  const label = eventStatusLabel(status);
  if (status === "running") return <Spinner size="sm" label={label} className="events-glyph" />;
  return <span className={`events-glyph events-glyph-${status}`} role="img" aria-label={label} title={label}>
    <Icon name={status === "completed" ? "check" : status === "failed" ? "x-circle" : "clock"} />
  </span>;
}

export function EventsPage({ onOpenSubject }: { onOpenSubject: (subject: PlatformEvent["subject"]) => void }) {
  const { events, loading, error, reload } = useEvents();
  return <Events events={events} onOpenSubject={onOpenSubject} loading={loading} error={error} onRetry={reload} />;
}

export function Events({ events, onOpenSubject, loading = false, error = null, onRetry }: {
  events: readonly PlatformEvent[];
  onOpenSubject: (subject: PlatformEvent["subject"]) => void;
  loading?: boolean;
  error?: string | null;
  onRetry?: () => void;
}) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [page, setPage] = useState(1);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedButton = useRef<HTMLButtonElement | null>(null);
  const scroller = useRef<HTMLDivElement | null>(null);
  const matching = filterEvents(events, query, status);
  const pages = Math.max(1, Math.ceil(matching.length / PAGE_SIZE));
  const current = Math.min(page, pages);
  const from = (current - 1) * PAGE_SIZE;
  const rows = matching.slice(from, from + PAGE_SIZE);
  const selected = matching.find(event => event.id === selectedId);
  const filtered = query !== "" || status !== "all";

  useEffect(() => {
    if ((scroller.current?.getBoundingClientRect().top ?? 0) < 0) scroller.current?.scrollIntoView({ block: "start" });
  }, [current]);

  const turnTo = (next: number) => { setPage(next); setSelectedId(null); };
  const closeDetails = () => {
    setSelectedId(null);
    selectedButton.current?.focus();
  };
  const resourceLink = (event: MouseEvent<HTMLAnchorElement>, subject: PlatformEvent["subject"]) => {
    event.stopPropagation();
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    onOpenSubject(subject);
  };
  const subjectLink = (subject: PlatformEvent["subject"]) => {
    const href = eventSubjectHref(subject);
    return href ? <a className="events-subject" href={href} onClick={event => resourceLink(event, subject)}>
      {subject.service ?? subject.name}
    </a> : <span title="This resource is no longer available">{subject.service ?? subject.name}</span>;
  };

  return <>
    <PageHeader>
      <PageHeaderTitle>Events</PageHeaderTitle>
    </PageHeader>
    {error && <div className="events-error" role="alert"><span>{error}</span>{onRetry && <Button size="sm" onClick={onRetry}>Retry</Button>}</div>}
    {!events.length && !loading && !error ? <Card><CardContent><EmptyState variant="informational">
      <EmptyStateTitle>No events yet</EmptyStateTitle>
      <EmptyStateDescription>Service and virtual machine activity will appear here.</EmptyStateDescription>
    </EmptyState></CardContent></Card> : <div className={`events-layout${selected ? " events-layout-open" : ""}`}>
      <div className="events-list">
        <div className="list-filters">
          <Search aria-label="Filter by service" placeholder="Search services or virtual machines..."
            value={query} onChange={event => { setQuery(event.target.value); turnTo(1); }} />
          <Select value={status} onValueChange={value => { setStatus(value); turnTo(1); }}>
            <SelectTrigger aria-label="Filter event status"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All statuses</SelectItem>
              {statuses.map(value => <SelectItem key={value} value={value}>{eventStatusLabel(value)}</SelectItem>)}
            </SelectContent>
          </Select>
          {filtered ? <Button size="sm" variant="ghost" onClick={() => { setQuery(""); setStatus("all"); turnTo(1); }}>Clear filters</Button> : null}
        </div>
        <TableFrame data-fill="false" className="events-table">
          <div className="table-scroll" ref={scroller}>
            <Table aria-label="Platform events">
              <TableHeader><TableRow>
                <TableHead className="events-status">Status</TableHead><TableHead>Action</TableHead><TableHead>Affected service</TableHead><TableHead>Actor</TableHead><TableHead>Last updated</TableHead>
              </TableRow></TableHeader>
              <TableBody>{!rows.length ? (loading ? [0, 1, 2].map(row => <TableRow key={row}>
                {Array.from({ length: 5 }, (_, cell) => <TableCell key={cell}><Skeleton /></TableCell>)}
              </TableRow>) : <TableRow><TableCell colSpan={5}>{error ? "Could not load events." : "No events match your filters."}</TableCell></TableRow>) : rows.map(event => <TableRow key={event.id} data-state={selected?.id === event.id ? "selected" : undefined}
                className={`events-row events-row-${event.status}`} onClick={click => {
                  if (click.target instanceof Element && click.target.closest("a")) return;
                  selectedButton.current = click.currentTarget.querySelector("button");
                  setSelectedId(event.id);
                }}>
                <TableCell className="events-status"><StatusGlyph status={event.status} /></TableCell>
                <TableCell>
                  <button className="events-action" aria-expanded={selected?.id === event.id}
                    aria-controls={selected?.id === event.id ? "event-details" : undefined}
                    onClick={click => { selectedButton.current = click.currentTarget; setSelectedId(event.id); }}>
                    {eventActionLabel(event)}
                  </button>
                  {event.status === "failed" && event.error ? <span className="events-secondary events-reason">{event.error.error}</span> : null}
                  <dl className="events-secondary events-row-details">
                    <dt>Actor</dt><dd><Actor event={event} /></dd>
                    <dt>Updated</dt><dd><Timestamp value={eventUpdatedAt(event)} inline /></dd>
                  </dl>
                </TableCell>
                <TableCell>{subjectLink(event.subject)}<span className="events-secondary">
                  {eventSubjectKindLabel(event.subject)}
                </span></TableCell>
                <TableCell><Actor event={event} /></TableCell>
                <TableCell><Timestamp value={eventUpdatedAt(event)} /></TableCell>
              </TableRow>)}</TableBody>
            </Table>
          </div>
          <p className="table-footer">
            <span role="status">{matching.length ? `${from + 1}–${from + rows.length} of ${matching.length}` : "0"} {matching.length === 1 ? "event" : "events"}{filtered ? ` · ${events.length} in total` : ""}</span>
            {pages > 1 && <Pagination aria-label="Event pages">
              <PaginationPrevious disabled={current === 1} onClick={() => turnTo(current - 1)} />
              {pageItems(current, pages).map((item, index) => item === "gap"
                ? <PaginationEllipsis key={`gap-${index}`} />
                : <PaginationPage key={item} active={item === current} aria-current={item === current ? "page" : undefined} onClick={() => turnTo(item)}>{item}</PaginationPage>)}
              <PaginationNext disabled={current === pages} onClick={() => turnTo(current + 1)} />
            </Pagination>}
          </p>
        </TableFrame>
      </div>
      {selected && <aside id="event-details" className="events-details" aria-labelledby="event-details-title"
        onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); closeDetails(); } }}>
        <Card>
          <CardHeader>
            <div className="events-details-top"><span className="t-caps muted">Event details</span><Button size="sm" variant="ghost" aria-label="Close event details" onClick={closeDetails}><Icon name="x" /></Button></div>
            <h2 id="event-details-title" className="section-title events-title"><StatusGlyph status={selected.status} />{eventActionLabel(selected)}</h2>
          </CardHeader>
          <CardContent>
            <p className="events-description">{selected.description}</p>
            {selected.error ? <div className="events-failure"><Failure failure={selected.error} /></div> : null}
            {selected.changes?.length ? <div className="events-changes"><Changes changes={selected.changes} /></div> : null}
            <dl className="events-properties">
              <dt>Status</dt><dd>{eventStatusLabel(selected.status)}</dd>
              <dt>Affected service</dt><dd>{subjectLink(selected.subject)}</dd>
              <dt>Resource</dt><dd>{selected.subject.kind === "virtual-machine" ? "Virtual machine" : selected.subject.name}</dd>
              <dt>Actor</dt><dd><Actor event={selected} /></dd>
              {selected.tool ? <><dt>MCP tool</dt><dd className="mono">{selected.tool}</dd></> : null}
              <dt>Last updated</dt><dd><Timestamp value={eventUpdatedAt(selected)} /></dd>
              <dt>Started</dt><dd>{selected.startedAt ? <Timestamp value={selected.startedAt} /> : "Not recorded"}</dd>
              <dt>Finished</dt><dd>{selected.finishedAt ? <Timestamp value={selected.finishedAt} /> : selected.status === "running" || selected.status === "pending" ? "Not finished" : "Not recorded"}</dd>
              <dt>Event ID</dt><dd className="mono">{selected.id}</dd>
            </dl>
          </CardContent>
        </Card>
      </aside>}
    </div>}
  </>;
}

function Actor({ event }: { event: PlatformEvent }) {
  const source = eventSource(event);
  return <span className="row-wrap">
    <span className={`mono${event.apiName ? "" : " muted"}`}>{event.apiName?.trim() || "system"}</span>
    {source ? <Badge variant="neutral">{source}</Badge> : null}
  </span>;
}
