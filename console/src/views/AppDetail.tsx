import { useRef, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
  Card,
  Pane,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Sparkline,
  Split,
  Splitter,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@momoi-labs/kiso-react";

import { AppForm, type Submission } from "../components/AppForm.js";
import { ContainerResources } from "../components/ContainerResources.js";
import { AppLogPane } from "../components/LogPane.js";
import { Failure } from "../components/Failure.js";
import { HttpStatus } from "../components/HttpStatus.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { useToast } from "../components/Toasts.js";
import { api, failureOf } from "../lib/api.js";
import { formatBytes } from "../lib/format.js";
import { hostnames, isCompose, statusTone } from "../lib/status.js";
import type { App, AppSample, Metrics, Report } from "../lib/types.js";
import { appTotals, containersFor, seriesFor } from "../lib/useMetrics.js";

export function AppDetail({
  app,
  dnsSuffix,
  metrics,
  reload,
  onRemoved,
}: {
  app: App;
  dnsSuffix: string;
  metrics: Metrics | null;
  reload: () => Promise<App[]>;
  onRemoved: () => void;
}) {
  const notify = useToast();
  const [confirming, setConfirming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const removalInFlight = useRef(false);
  const samples = seriesFor(metrics, app.id);
  const containers = containersFor(metrics, app.id);
  const totals = appTotals(metrics, app.id);

  const canStop = app.status === "running" || app.status === "failed";
  const canStart = app.status === "stopped" || app.status === "failed";

  async function lifecycle(verb: string) {
    if (removalInFlight.current) return;
    try {
      const res = await api(`/apps/id/${encodeURIComponent(app.id)}/${verb}`, { method: "POST" });
      if (!res.ok) {
        notify("danger", `Could not ${verb} ${app.name}`, await failureOf(res));
      } else {
        const updated = (await res.json()) as App;
        notify(
          updated.status === "failed" ? "danger" : "success",
          `${app.name} is ${updated.status}`,
          updated.status === "failed" ? updated.last_error : undefined,
        );
      }
    } catch (cause) {
      notify("danger", `Could not ${verb} ${app.name}`, (cause as Error).message);
    }
    await reload();
  }

  async function save(body: Submission): Promise<Report | null> {
    if (removalInFlight.current) return { error: "Application removal is in progress.", caused_by: [] };
    let failure: Report | null = null;
    try {
      const res = await api(`/apps/id/${encodeURIComponent(app.id)}`, {
        method: "PUT",
        body: JSON.stringify(body),
      });
      if (res.status === 202) {
        notify("success", "Changes saved", {
          error: `${body.name} is redeploying.`,
          caused_by: [],
        });
      } else if (res.ok) {
        notify("success", "Changes saved", { error: `${body.name} was updated.`, caused_by: [] });
      } else {
        failure = await failureOf(res);
      }
    } catch (cause) {
      failure = { error: "Network error", caused_by: [(cause as Error).message] };
    }

    // Reload either way: a failed redeploy still changed the stored record.
    const next = await reload();
    if (!failure) return null;

    // The record already carries the reason when the deploy itself failed, and
    // the detail renders it. Only errors that never reached the database — a
    // rejected name, a clash, a refused file — need saying here.
    const saved = next.find((candidate) => candidate.id === app.id);
    if (saved?.last_error && !failure.error.startsWith("invalid")) return null;
    return failure;
  }

  async function remove() {
    if (removalInFlight.current) return;
    removalInFlight.current = true;
    setConfirming(false);
    setRemoving(true);
    notify("neutral", "Removing application...");
    try {
      const res = await api(`/apps/${encodeURIComponent(app.name)}`, { method: "DELETE" });
      if (!res.ok) {
        const failure = await failureOf(res);
        notify("danger", `Could not remove ${app.name}`, failure);
        setRemoving(false);
        removalInFlight.current = false;
        return;
      }
      notify("success", "Application removed", {
        error: `${app.name} and its containers are gone. Its data stays on the Host.`,
        caused_by: [],
      });
    } catch (cause) {
      const failure = { error: "Network error", caused_by: [(cause as Error).message] };
      notify("danger", `Could not remove ${app.name}`, failure);
      setRemoving(false);
      removalInFlight.current = false;
      return;
    }
    setRemoving(false);
    removalInFlight.current = false;
    onRemoved();
    await reload();
  }

  return (
    <>
      <div className="between">
        <PageHeader>
          <PageHeaderTitle>{app.name}</PageHeaderTitle>
          <PageHeaderDescription>
            {hostnames(app).map((host, index) => (
              <span key={host}>
                {index ? " · " : null}
                <a href={`https://${host}`} target="_blank" rel="noreferrer" className="mono">
                  {host}
                </a>
              </span>
            ))}
          </PageHeaderDescription>
          {samples ? <Glance samples={samples} /> : null}
        </PageHeader>
        <div className="lifecycle">
          <StatusBadge tone={statusTone(app.status)}>{app.status}</StatusBadge>
          <HttpStatus id={app.id} status={app.status} />
          <Button size="sm" disabled={removing || !canStart} onClick={() => void lifecycle("start")}>
            Start
          </Button>
          <Button size="sm" disabled={removing || !canStop} onClick={() => void lifecycle("stop")}>
            Stop
          </Button>
          <Button
            size="sm"
            disabled={removing || app.status !== "running"}
            onClick={() => void lifecycle("restart")}
          >
            Restart
          </Button>
        </div>
      </div>

      {app.last_error ? (
        <Failure
          failure={app.last_error}
          actionLabel="Retry"
          onAction={() =>
            (document.getElementById("app-form") as HTMLFormElement | null)?.requestSubmit()
          }
        />
      ) : null}

      <Card className="detail-panel">
        <Split>
          <Pane>
            <Tabs defaultValue="configuration" className="pane-tabs">
              <TabsList className="tabs">
                <TabsTrigger value="configuration">Configuration</TabsTrigger>
                <TabsTrigger value="resources">Resources</TabsTrigger>
              </TabsList>
              <TabsContent value="configuration">
                <AppForm
                  app={app}
                  dnsSuffix={dnsSuffix}
                  onSubmit={save}
                  removing={removing}
                  onRemove={() => {
                    if (!removing && !removalInFlight.current) {
                      setConfirming(true);
                    }
                  }}
                />
              </TabsContent>
              <TabsContent value="resources">
                <ContainerResources
                  samples={samples}
                  services={app.services ?? []}
                  containers={containers}
                  totals={totals}
                  intervalSeconds={metrics?.interval_seconds ?? 10}
                />
              </TabsContent>
            </Tabs>
          </Pane>
          <Splitter defaultSize={42} min={25} max={70} aria-label="Resize the form and the logs" />
          <Pane className="pane-logs">
            <AppLogPane id={app.id} />
          </Pane>
        </Split>
      </Card>

      {/*
        Replaces window.confirm, which cannot be styled and says the hostname
        out loud. Only the destructive button removes anything.
      */}
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove application</AlertDialogTitle>
          </AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              <code>{app.name}</code> and its {isCompose(app) ? "containers" : "container"} will be removed.
              Named volumes and the data directory are kept on the Host.
            </AlertDialogDescription>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="btn-danger" onClick={() => void remove()}>
              Remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/**
 * One line of numbers under the hostname: the glance. The detail screen is
 * for configuration and logs, so the history lives in the Resources tab
 * rather than in a card that pushes the panel down (ADR-0020).
 */
function Glance({ samples }: { samples: AppSample[] }) {
  const latest = samples[samples.length - 1];
  const values = samples.map((sample) => sample.cpu_percent);
  const memory = samples.map((sample) => sample.memory_bytes);
  return (
    <div className="glance">
      <span>
        <span className="k">CPU</span> <b>{latest.cpu_percent.toFixed(2)}%</b>
      </span>
      <Sparkline values={values} height={18} />
      <span className="sep">·</span>
      <span>
        <span className="k">Memory</span> <b>{formatBytes(latest.memory_bytes)}</b>
      </span>
      <Sparkline values={memory} height={18} />
      <span className="sep">·</span>
      <span>
        <span className="k">Net</span>{" "}
        <b>
          ↓ {formatBytes(latest.rx_bytes)} · ↑ {formatBytes(latest.tx_bytes)}
        </b>
      </span>
    </div>
  );
}
