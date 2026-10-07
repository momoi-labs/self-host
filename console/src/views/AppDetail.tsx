import { Fragment, Suspense, lazy, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  Lifecycle,
  StatusBadge,
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
  Checkbox,
  Label,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Skeleton,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@momoi-labs/kiso-react";

import { AppForm, type Submission } from "../components/AppForm.js";
import { Failure } from "../components/Failure.js";
import { Glance } from "../components/Glance.js";
import { NativeSummary } from "../components/NativeSummary.js";
import { DeploymentHistory, DeploymentReadiness } from "../components/DeploymentHistory.js";
import { HttpStatus } from "../components/HttpStatus.js";
import { GitBuildRun } from "../components/GitBuildRun.js";
import { GitUpdateDialog } from "../components/GitUpdateDialog.js";
import { AppLogPane } from "../components/LogPane.js";
import { useToast } from "../components/Toasts.js";
import { api, failureOf } from "../lib/api.js";
import { settingsQuery } from "../lib/queries.js";
import { hostnames, isCompose, statusTone } from "../lib/status.js";
import { waitForTask } from "../lib/tasks.js";
import type { App, GitSource, Metrics, Report } from "../lib/types.js";
import { fetchEvents, useEvents } from "../lib/useEvents.js";
import { seriesFor } from "../lib/useMetrics.js";
import { useMediaQuery } from "../lib/useMediaQuery.js";

// xterm is a third of the console's JavaScript and only the Terminal tab needs
// it, so the chunk arrives when the Operator opens that tab.
const Terminal = lazy(() => import("../components/Terminal.js").then((module) => ({ default: module.Terminal })));

export function AppDetail({
  app,
  formRevision,
  dnsSuffix,
  metrics,
  reload,
  onRemoved,
}: {
  app: App;
  formRevision: string;
  dnsSuffix: string;
  metrics: Metrics | null;
  reload: () => Promise<App[]>;
  onRemoved: () => void;
}) {
  const notify = useToast();
  const queryClient = useQueryClient();
  const wide = useMediaQuery("(min-width: 1024px)");
  const native = app.runtime?.kind === "native" ? app.runtime : null;
  const [confirming, setConfirming] = useState(false);
  /** The restart waiting for confirmation, or null. */
  const [restart, setRestart] = useState<{ pull: boolean } | null>(null);
  const [removing, setRemoving] = useState(false);
  const removalInFlight = useRef(false);
  const samples = seriesFor(metrics, app.id);
  const [tab, setTab] = useState(app.status === "pending" && native ? "logs" : app.git && app.status === "pending" ? "last-update" : native || app.git_build ? "summary" : "configuration");
  const [openedTerminal, setOpenedTerminal] = useState(false);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [building, setBuilding] = useState(false);
  const buildInFlight = useRef(false);
  const [buildTask, setBuildTask] = useState<string | null>(app.task_id ?? null);
  const { events, error: eventsError } = useEvents();
  const latestBuild = events.filter((event) => event.subject.id === app.id && (event.id === buildTask || event.description.startsWith("Git build")))
    .sort((left, right) => Date.parse(right.occurredAt) - Date.parse(left.occurredAt))[0] ?? null;
  const lastBuild = building && buildTask && !events.some((event) => event.id === buildTask) ? null : latestBuild;
  const gitBusy = Boolean(app.git && (building || lastBuild?.status === "running" || lastBuild?.status === "pending"));

  const canStop = app.status === "running" || app.status === "failed";
  const canStart = app.status === "stopped" || app.status === "failed";

  // The API queues the action and answers with its task; the outcome comes
  // from the task's event, which keeps the same id from start to finish.
  async function lifecycle(verb: string, body?: object) {
    if (removalInFlight.current || buildInFlight.current || gitBusy) return;
    try {
      const res = await api(`/apps/id/${encodeURIComponent(app.id)}/${verb}`, {
        method: "POST",
        body: body ? JSON.stringify(body) : undefined,
      });
      if (!res.ok) {
        notify("danger", `Could not ${verb} ${app.name}`, await failureOf(res));
      } else {
        const accepted = (await res.json()) as App;
        if (!accepted.task_id) throw new Error("The API did not name the task.");
        notify("neutral", `${app.name}: ${verb} queued`);
        const outcome = await waitForTask(accepted.task_id, { events: fetchEvents });
        const next = await reload();
        const updated = next.find((candidate) => candidate.id === app.id);
        if (outcome.status === "completed") {
          notify("success", `${app.name} is ${updated?.status ?? "updated"}`);
        } else {
          notify("danger", `Could not ${verb} ${app.name}`, outcome.error ?? undefined);
        }
        return;
      }
    } catch (cause) {
      notify("danger", `Could not ${verb} ${app.name}`, (cause as Error).message);
    }
    await reload();
  }

  /** Opens the restart confirmation with the Platform's choice to pull. */
  async function askRestart() {
    if (native) { setRestart({ pull: false }); return; }
    const settings = await queryClient.fetchQuery(settingsQuery).catch(() => null);
    setRestart({ pull: settings?.pullNewerImages.effective ?? false });
  }

  async function save(body: Submission): Promise<Report | null> {
    if (removalInFlight.current) return { error: "Application removal is in progress.", caused_by: [] };
    if (app.git) return runGitBuild(body);
    let failure: Report | null = null;
    try {
      const res = await api(`/apps/id/${encodeURIComponent(app.id)}`, {
        method: "PUT",
        body: JSON.stringify(body),
      });
      if (res.status === 202) {
        if (native) setTab("logs");
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

  async function runGitBuild(body: object): Promise<Report | null> {
    if (removalInFlight.current || buildInFlight.current || gitBusy) return { error: "An Application operation is already in progress.", caused_by: [] };
    buildInFlight.current = true;
    setBuilding(true);
    let failure: Report | null = null;
    try {
      const response = await api(`/apps/id/${encodeURIComponent(app.id)}`, { method: "PUT", body: JSON.stringify(body) });
      if (!response.ok) failure = await failureOf(response);
      else {
        const accepted = await response.json() as App;
        if (!accepted.task_id) throw new Error("The API did not name the build task.");
        setBuildTask(accepted.task_id);
        setTab("last-update");
        const outcome = await waitForTask(accepted.task_id, { events: fetchEvents });
        if (outcome.status === "failed") failure = outcome.error ?? { error: "The Git build failed.", caused_by: [] };
        else notify("success", "Build and deployment completed");
      }
    } catch (cause) {
      failure = { error: "Could not follow the Git build", caused_by: [(cause as Error).message] };
    } finally {
      buildInFlight.current = false;
      setBuilding(false);
      await reload();
    }
    return failure;
  }

  async function deployUpdate(source: GitSource, revision: string, expected: string) {
    const failure = await runGitBuild({ git: source, refresh_source: true, source_revision: revision, expected_git_revision: expected });
    if (failure) notify("danger", "Could not update application", failure);
  }

  async function rebuildCurrent() {
    const failure = await runGitBuild({ refresh_source: false });
    if (failure) notify("danger", "Could not rebuild application", failure);
  }

  async function remove() {
    if (removalInFlight.current || buildInFlight.current || gitBusy) return;
    removalInFlight.current = true;
    setConfirming(false);
    setRemoving(true);
    notify("neutral", "Removing application...");
    try {
      const res = await api(`/apps/id/${encodeURIComponent(app.id)}`, { method: "DELETE" });
      if (!res.ok) {
        const failure = await failureOf(res);
        notify("danger", `Could not remove ${app.name}`, failure);
        setRemoving(false);
        removalInFlight.current = false;
        return;
      }
      const { task_id } = (await res.json()) as { task_id: string };
      const outcome = await waitForTask(task_id, { events: fetchEvents });
      if (outcome.status === "failed") {
        notify("danger", `Could not remove ${app.name}`, outcome.error ?? undefined);
        setRemoving(false);
        removalInFlight.current = false;
        await reload();
        return;
      }
      notify("success", "Application removed", {
        error: native ? `${app.name} and its processes are gone. Its data stays on the Host.` : `${app.name} and its containers are gone. Its data stays on the Host.`,
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
        <PageHeader className={wide ? "grow" : undefined}>
          <PageHeaderTitle>{app.name}</PageHeaderTitle>
          {app.hostname ? <PageHeaderDescription>
            {hostnames(app).map((host, index) => (
              <span key={host}>
                {index ? " · " : null}
                <a href={`https://${host}`} target="_blank" rel="noreferrer" className="mono">
                  {host}
                </a>
              </span>
            ))}
          </PageHeaderDescription> : null}
          {samples ? <Glance samples={samples} /> : null}
        </PageHeader>
        <Lifecycle
          status={
            <>
              <StatusBadge tone={native && app.status === "pending" ? "success" : statusTone(app.status)} pulse={Boolean(native && app.status === "pending")}>{native && app.status === "pending" ? "Preparing" : app.status}</StatusBadge>
              {app.publication?.kind !== "unpublished" ? <HttpStatus id={app.id} status={app.status} /> : null}
              {app.runtime?.kind !== "native" ? <DeploymentReadiness app={app} /> : null}
              {gitBusy ? <StatusBadge tone="success" pulse>{lastBuild?.status === "pending" ? "Queued" : "Building"}</StatusBadge> : null}
            </>
          }
          actions={
            <>
              <Button size="sm" disabled={removing || gitBusy || !canStart} onClick={() => void lifecycle("start")}>
                Start
              </Button>
              <Button size="sm" disabled={removing || gitBusy || !canStop} onClick={() => void lifecycle("stop")}>
                Stop
              </Button>
              <Button
                size="sm"
                disabled={removing || gitBusy || app.status !== "running"}
                onClick={() => void askRestart()}
              >
                Restart
              </Button>
              {app.git && app.git_build ? <Button size="sm" disabled={removing || gitBusy} onClick={() => setCheckingUpdate(true)}>Update</Button> : null}
            </>
          }
          destructive={
            <Button
              size="sm"
              variant="ghost"
              className="btn-danger-ghost"
              disabled={removing || gitBusy}
              onClick={() => {
                if (!removing && !removalInFlight.current) setConfirming(true);
              }}
            >
              {removing ? "Removing..." : "Remove application"}
            </Button>
          }
        />
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

      {/* One card for the configuration and for whatever the Application
          prints, the way the machine's screen already worked. */}
      <Card className="detail-tabs">
        <Tabs
          value={tab}
          onValueChange={(next) => {
            setTab(next);
            if (next === "terminal") setOpenedTerminal(true);
          }}
        >
          <TabsList aria-label="Application details">
            {native || app.git_build ? <TabsTrigger value="summary">Summary</TabsTrigger> : null}
            <TabsTrigger value="configuration">Configuration</TabsTrigger>
            {app.git && (lastBuild || building || app.status === "pending") ? <TabsTrigger value="last-update">Last update</TabsTrigger> : null}
            {app.runtime?.kind !== "native" ? <TabsTrigger value="deployments">Deployments</TabsTrigger> : null}
            <TabsTrigger value="logs">Logs</TabsTrigger>
            <TabsTrigger value="terminal">Terminal</TabsTrigger>
          </TabsList>
          {native ? <TabsContent value="summary"><NativeSummary app={app} /></TabsContent> : null}
          {app.git_build ? <TabsContent value="summary">
            <dl className="summary-facts">
              {app.git ? <><dt>Repository</dt><dd>{app.git.repository}</dd><dt>Branch or tag</dt><dd><code>{app.git.git_ref}</code></dd></> : null}
              <dt>Deployed commit</dt>
              <dd><code>{app.git_build.revision}</code></dd>
              <dt>Build result</dt>
              <dd><StatusBadge tone={app.git_build.status === "completed" ? "success" : "neutral"}>{app.git_build.status}</StatusBadge></dd>
              {Object.entries(app.git_build.images).map(([service, image]) => <Fragment key={service}>
                <dt>{service} image</dt><dd><code>{image}</code></dd>
              </Fragment>)}
            </dl>
            <Button size="sm" disabled={removing || gitBusy} onClick={() => void rebuildCurrent()}>Rebuild current version</Button>
          </TabsContent> : null}
          <TabsContent value="configuration">
            <AppForm key={formRevision} app={app} dnsSuffix={dnsSuffix} onSubmit={save} onReload={reload} />
          </TabsContent>
          {app.git ? <TabsContent value="last-update" className="detail-run">
            <GitBuildRun event={lastBuild} error={eventsError} busy={gitBusy} onRetry={() => setTab("configuration")} />
          </TabsContent> : null}
          {app.runtime?.kind !== "native" ? <TabsContent value="deployments"><DeploymentHistory app={app} reload={reload} /></TabsContent> : null}
          <TabsContent value="logs" className="detail-logs">
            {/* The panel is one row tall; the Application's log brings a
                container picker above it, so the two share a wrapper. */}
            <div className="detail-log-pane"><AppLogPane id={app.id} native={!!native} /></div>
          </TabsContent>
          <TabsContent value="terminal" className="detail-pane" forceMount hidden={tab !== "terminal"}>
            {openedTerminal ? (
              <Suspense fallback={<Skeleton className="terminal-loading" />}>
                <Terminal id={app.id} native={!!native} account={native?.account} />
              </Suspense>
            ) : null}
          </TabsContent>
        </Tabs>
      </Card>

      {checkingUpdate ? <GitUpdateDialog key={`${app.id}:${app.git_build?.revision}:${JSON.stringify(app.git)}`} app={app} onClose={() => setCheckingUpdate(false)} onDeploy={deployUpdate} /> : null}

      {/*
        The checkbox starts where the Platform setting is, and this restart
        can go the other way without touching the setting.
      */}
      <AlertDialog open={restart !== null} onOpenChange={(open) => { if (!open) setRestart(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Restart application</AlertDialogTitle>
          </AlertDialogHeader>
          <div className="dialog-body stack">
            <AlertDialogDescription>
              <code>{app.name}</code> restarts its {native ? "process tree" : isCompose(app) ? "containers" : "container"}.
            </AlertDialogDescription>
            {!native ? <div className="field">
              <div className="check">
                <Checkbox id="restart-pull" checked={restart?.pull ?? false}
                  onCheckedChange={(checked) => setRestart({ pull: checked === true })}
                  aria-describedby="restart-pull-hint" />
                <Label htmlFor="restart-pull">Pull newer images</Label>
              </div>
              <p id="restart-pull-hint" className="muted t-label">
                Pulls each image from its registry, then recreates the containers. If a pull fails,
                nothing restarts. Images built on this Host are not pulled.
              </p>
            </div> : null}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="primary"
              onClick={() => {
                const pull = restart?.pull ?? false;
                setRestart(null);
                void lifecycle("restart", { pull });
              }}
            >
              Restart
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

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
              <code>{app.name}</code> and its {native ? "processes and Application Account" : isCompose(app) ? "containers" : "container"} will be removed.
              {native ? " The Application data stays on the Host with access revoked." : " Named volumes and the data directory are kept on the Host."}
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
