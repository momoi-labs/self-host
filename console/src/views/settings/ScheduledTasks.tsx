import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Button, GridPane, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, StatusBadge, Switch,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf, readJson } from "../../lib/api.js";
import { formatAgo } from "../../lib/format.js";
import { useToast } from "../../components/Toasts.js";
import type { PaneProps } from "./General.js";

type Every = "day" | "week";

/** A built-in schedule, as `GET /schedules` returns it. Hours are UTC. */
type Schedule = {
  id: string;
  title: string;
  description: string;
  enabled: boolean;
  every: Every;
  hour: number;
  last_run_at: string | null;
  last_task: string | null;
  running: boolean;
  next_run_at: string | null;
};

const noSchedules: Schedule[] = [];
const hours = Array.from({ length: 24 }, (_, hour) => hour);

/** A UTC hour in this browser's time, so the Operator picks the hour they live in. */
function localHour(utcHour: number): string {
  const at = new Date();
  at.setUTCHours(utcHour, 0, 0, 0);
  return at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/**
 * The Scheduled tasks card of Settings: work the Platform runs on its own,
 * each turned on or off, on a day or a week, at an hour. A run is a task like
 * any other, so its outcome is in Events (#201).
 */
export function ScheduledTasks({ id, size }: PaneProps) {
  const notify = useToast();
  const [busy, setBusy] = useState<string | null>(null);
  const { data: schedules = noSchedules, refetch } = useQuery({
    queryKey: ["schedules"],
    queryFn: ({ signal }) => readJson<Schedule[]>("/schedules", signal),
    // A run started here, or by the clock, shows up without a reload.
    refetchInterval: 15_000,
  });

  async function save(schedule: Schedule, change: Partial<Pick<Schedule, "enabled" | "every" | "hour">>) {
    setBusy(schedule.id);
    try {
      const response = await api(`/schedules/${encodeURIComponent(schedule.id)}`, {
        method: "PUT",
        body: JSON.stringify({ enabled: schedule.enabled, every: schedule.every, hour: schedule.hour, ...change }),
      });
      if (!response.ok) notify("danger", "Could not save the schedule", await failureOf(response));
      await refetch();
    } catch (cause) {
      notify("danger", "Could not save the schedule", asReport(cause));
    } finally {
      setBusy(null);
    }
  }

  async function runNow(schedule: Schedule) {
    setBusy(schedule.id);
    try {
      const response = await api(`/schedules/${encodeURIComponent(schedule.id)}/run`, { method: "POST" });
      if (!response.ok) notify("danger", `Could not run ${schedule.title}`, await failureOf(response));
      else notify("success", `${schedule.title} started`, { error: "Its outcome is in Events.", caused_by: [] });
      await refetch();
    } catch (cause) {
      notify("danger", `Could not run ${schedule.title}`, asReport(cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <GridPane id={id} size={size} title="Scheduled tasks">
      <p className="muted t-label">Work the Platform runs on its own. Each run is a task, and its outcome is in Events.</p>
      <div className="table-wrap">
        <div className="table-scroll">
          <Table aria-label="Scheduled tasks">
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Task</TableHead>
                <TableHead scope="col">On</TableHead>
                <TableHead scope="col">When</TableHead>
                <TableHead scope="col">Last run</TableHead>
                <TableHead scope="col">Next run</TableHead>
                <TableHead scope="col" className="col-tight" aria-label="Actions" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {schedules.map((schedule) => (
                <TableRow key={schedule.id}>
                  <TableCell>
                    <span className="schedule-task">
                      <span>{schedule.title}</span>
                      <span className="muted t-label">{schedule.description}</span>
                    </span>
                  </TableCell>
                  <TableCell>
                    <Switch aria-label={`Run ${schedule.title} on a schedule`} checked={schedule.enabled}
                      disabled={busy === schedule.id} onCheckedChange={(checked) => void save(schedule, { enabled: checked === true })} />
                  </TableCell>
                  <TableCell>
                    <span className="schedule-when">
                      <Select value={schedule.every} disabled={busy === schedule.id} onValueChange={(every) => void save(schedule, { every: every as Every })}>
                        <SelectTrigger aria-label="How often"><SelectValue /></SelectTrigger>
                        <SelectContent>
                          <SelectItem value="day">Daily</SelectItem>
                          <SelectItem value="week">Weekly, Sunday</SelectItem>
                        </SelectContent>
                      </Select>
                      <Select value={String(schedule.hour)} disabled={busy === schedule.id} onValueChange={(hour) => void save(schedule, { hour: Number(hour) })}>
                        <SelectTrigger aria-label="At"><SelectValue /></SelectTrigger>
                        <SelectContent>
                          {hours.map((hour) => <SelectItem key={hour} value={String(hour)}>{localHour(hour)}</SelectItem>)}
                        </SelectContent>
                      </Select>
                    </span>
                  </TableCell>
                  <TableCell>
                    {schedule.running ? <StatusBadge tone="success" pulse>Running</StatusBadge>
                      : schedule.last_run_at ? formatAgo(schedule.last_run_at) : <span className="muted">Never</span>}
                  </TableCell>
                  <TableCell>
                    {schedule.next_run_at ? new Date(schedule.next_run_at).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" })
                      : <span className="muted">Off</span>}
                  </TableCell>
                  <TableCell className="col-tight">
                    <Button size="sm" variant="ghost" disabled={busy === schedule.id || schedule.running} onClick={() => void runNow(schedule)}>Run now</Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </div>
    </GridPane>
  );
}
