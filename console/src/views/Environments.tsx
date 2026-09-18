import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Badge,
  Button,
  Card,
  EmptyState,
  EmptyStateActions,
  EmptyStateTitle,
  Form,
  FormActions,
  FormField,
  Label,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  StepBar,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@momoi-labs/kiso-react";

import { Dependencies } from "../components/Dependencies.js";
import { Failure } from "../components/Failure.js";
import { Glance } from "../components/Glance.js";
import { LastRun } from "../components/LastRun.js";
import { Lifecycle } from "../components/Lifecycle.js";
import { ShellEditor } from "../components/ShellEditor.js";
import { Step, Steps } from "../components/Steps.js";
import { LogSurface } from "../components/LogSurface.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { Terminal } from "../components/Terminal.js";
import { api, asReport, failureOf } from "../lib/api.js";
import {
  customImageTemplate,
  customImageTemplates,
} from "../lib/customImageTemplates.js";
import { isVersion } from "../lib/dependencies.js";
import {
  NOUNS,
  TITLES,
  formatSeconds,
  parseRun,
  phase,
  position,
  seconds,
  stepLabel,
  stepViews,
  type Run,
} from "../lib/runSteps.js";
import { machineSeriesFor } from "../lib/useMetrics.js";
import type {
  Environment,
  EnvironmentConfig,
  Metrics,
  Report,
} from "../lib/types.js";


const demoConfig = (name: string): EnvironmentConfig => ({
  name,
  cpus: 4,
  memory_gib: 8,
  disk_gib: 40,
  ssh_public_key: "ssh-ed25519 AAAA... operator@host",
  recipe: {
    name: "T3 Code",
    template_id: "t3-code",
    dependencies: structuredClone(
      customImageTemplate("t3-code")?.dependencies ?? [],
    ),
    setup: structuredClone(customImageTemplate("t3-code")?.setup ?? []),
    build_checks: structuredClone(
      customImageTemplate("t3-code")?.buildChecks ?? [],
    ),
  },
  command:
    "t3 serve --mode web --host 0.0.0.0 --port 3000 --base-dir /home/dev/.local/state/t3",
  web_port: 3000,
});
const demoEnvironments: Environment[] = [
  {
    id: "demo-running",
    config: demoConfig("t3-workbench"),
    state: "running",
    service_ready: true,
    operation: null,
    log: "Virtual machine booted\nSSH is ready\nService is responding",
    ssh_command: "ssh operator@host -p 2222",
    tunnel_command: "ssh -N -L 3000:127.0.0.1:3000 operator@host -p 2222",
    web_url: "http://127.0.0.1:3000",
    installed_versions: {
      node: "24.21.0",
      "npm:t3": "0.0.42",
      "claude-code": "2.1.274",
      "npm:@openai/codex": "0.154.0",
    },
  },
  {
    id: "demo-starting",
    config: demoConfig("research-box"),
    state: "running",
    service_ready: false,
    operation: {
      action: "bootstrap",
      status: "running",
      step: "health",
    },
    log: "Virtual machine booted\nInstalling tools\nChecking service readiness...",
    ssh_command: "ssh operator@host -p 2223",
    tunnel_command: "ssh -N -L 3001:127.0.0.1:3000 operator@host -p 2223",
    web_url: "http://127.0.0.1:3001",
    installed_versions: {},
  },
  {
    id: "demo-failed",
    config: demoConfig("broken-bootstrap"),
    state: "stopped",
    service_ready: false,
    operation: {
      action: "bootstrap",
      status: "failed",
      step: "tools",
      error: {
        error: "Provisioning failed at tools. The output above is the machine's own account.",
        caused_by: ["npm:t3 could not be downloaded"],
      },
    },
    log: "Virtual machine created\nInstalling mise dependencies\nERROR network request failed",
    ssh_command: "ssh operator@host -p 2224",
    tunnel_command: "ssh -N -L 3002:127.0.0.1:3000 operator@host -p 2224",
    web_url: "http://127.0.0.1:3002",
    installed_versions: {},
  },
];

/// The Host's event log for the demo machines, so the demo has a last run
/// to draw: a bootstrap that reached the end, one still going, and one that
/// stopped at its tools. Each line is seconds after the run started.
function demoLog(startedAt: string, lines: [number, string][]) {
  const start = Date.parse(startedAt);
  return lines
    .map(([after, text]) => {
      const at = new Date(start + after * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
      return `${at} bootstrap ${text}`;
    })
    .join("\n");
}
const demoBootstrap: [number, string][] = [
  [0, "--- bootstrap started ---"],
  [0, "starting"],
  [6, "provisioning"],
  [7, "SF_STEP system-packages"],
  [8, "Get:1 http://archive.ubuntu.com/ubuntu noble InRelease [256 kB]"],
  [12, "Get:2 http://archive.ubuntu.com/ubuntu noble-updates InRelease [126 kB]"],
  [40, "Setting up curl (8.5.0-2ubuntu10.6) ..."],
  [54, "Setting up git (1:2.43.0-1ubuntu7.2) ..."],
  [55, "SF_STEP user-and-ssh"],
  [57, "SF_STEP mise"],
  [58, "mise 2025.9.1 installed to /home/dev/.local/bin/mise"],
  [66, "SF_STEP tools"],
];
const demoEvents: Record<string, string> = {
  "demo-running": demoLog("2026-09-18T14:00:00Z", [
    ...demoBootstrap,
    [178, "SF_STEP setup"],
    [182, "SF_STEP checks"],
    [193, "SF_STEP service"],
    [196, "SF_STEP health"],
    [203, "SF_STEP ready"],
    [203, "--- bootstrap succeeded ---"],
  ]),
  "demo-starting": demoLog("2026-09-18T16:00:00Z", [
    ...demoBootstrap,
    [178, "SF_STEP setup"],
    [182, "SF_STEP checks"],
    [193, "SF_STEP service"],
    [196, "SF_STEP health"],
    [196, "curl: (7) Failed to connect to 127.0.0.1 port 3000 after 0 ms: Couldn't connect to server"],
    [199, "curl: (7) Failed to connect to 127.0.0.1 port 3000 after 0 ms: Couldn't connect to server"],
    [202, "curl: (56) Recv failure: Connection reset by peer"],
  ]),
  "demo-failed": demoLog("2026-09-18T15:00:00Z", [
    ...demoBootstrap,
    [66, "mise install node@24 ... done"],
    [100, "mise install npm:t3@latest"],
    [178, "npm ERR! network request to https://registry.npmjs.org/t3 failed"],
    [178, "mise ERROR failed to install npm:t3@latest"],
    [178, "SF_STEP failed:tools"],
    [179, "--- bootstrap failed: Provisioning failed at tools. ---"],
  ]),
};

/// A machine starts empty. A template is an offer, not a default, so nothing
/// is installed and no service runs until the Operator asks for one.
function freshConfig(): EnvironmentConfig {
  return {
    name: "",
    cpus: 4,
    memory_gib: 8,
    disk_gib: 40,
    ssh_public_key: "",
    recipe: {
      name: "",
      template_id: null,
      dependencies: [],
      setup: [],
      build_checks: [],
    },
    command: "",
    web_port: 0,
  };
}
function tone(state: string) {
  return state === "running"
    ? ("success" as const)
    : state === "failed"
      ? ("danger" as const)
      : ("neutral" as const);
}
function actionBusy(environment: Environment) {
  return !!environment.operation && environment.operation.status === "running";
}
function equalConfig(a: EnvironmentConfig, b: EnvironmentConfig) {
  return JSON.stringify(a) === JSON.stringify(b);
}
function installedVersions(value: unknown): [string, string][] {
  if (!value || typeof value !== "object" || Array.isArray(value))
    return value == null ? [] : [["details", readableVersion(value)]];
  return Object.entries(value).map(([name, version]) => [
    name,
    readableVersion(version),
  ]);
}
function readableVersion(value: unknown): string {
  if (typeof value === "string" || typeof value === "number")
    return String(value);
  if (Array.isArray(value))
    return value.map(readableVersion).filter(Boolean).join(", ");
  if (value && typeof value === "object") {
    const version = (value as { version?: unknown }).version;
    if (typeof version === "string" || typeof version === "number")
      return String(version);
  }
  return JSON.stringify(value);
}
/**
 * The Platform omits an empty list rather than sending `[]`, so a machine with
 * no build checks arrives without the field at all. Put the empty lists back
 * once, here, so nothing downstream has to ask whether they are there.
 */
function normalizeConfig(value: EnvironmentConfig): EnvironmentConfig {
  return {
    ...value,
    recipe: {
      ...value.recipe,
      dependencies: (value.recipe?.dependencies ?? []).map((dependency) => ({
        ...dependency,
        allow_builds: dependency.allow_builds ?? [],
      })),
      setup: value.recipe?.setup ?? [],
      build_checks: value.recipe?.build_checks ?? [],
    },
  };
}

function normalizeEnvironment(value: Environment): Environment {
  return {
    ...value,
    config: normalizeConfig(value.config),
    applied_config: value.applied_config
      ? normalizeConfig(value.applied_config)
      : value.applied_config,
    operation: value.operation
      ? {
          ...value.operation,
          step: value.operation.step || value.operation.status,
        }
      : null,
    installed_versions: Object.fromEntries(
      installedVersions(value.installed_versions),
    ),
  };
}

export function Environments({
  mode,
  selected,
  metrics,
  onOpen,
}: {
  mode: "new" | "detail";
  selected: string | null;
  metrics: Metrics | null;
  onOpen: (id: string | null) => void;
}) {
  const demo =
    new URLSearchParams(location.search).get("demo") === "environments";
  const [records, setRecords] = useState<Environment[]>(
    demo ? demoEnvironments : [],
  );
  const [failure, setFailure] = useState<Report | null>(null);
  const [config, setConfig] = useState<EnvironmentConfig>(freshConfig);
  const [savedConfig, setSavedConfig] = useState<EnvironmentConfig | null>(
    null,
  );
  const [appliedConfig, setAppliedConfig] = useState<EnvironmentConfig | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<Environment | null>(null);
  const [logs, setLogs] = useState("");
  const [events, setEvents] = useState("");
  const [tab, setTab] = useState("summary");
  const [confirmName, setConfirmName] = useState("");
  const requestId = useRef<string | null>(null);
  const opened = useRef<string | null>(null);
  const wasRunning = useRef(false);
  const editorInitialized = useRef<string | null>(null);
  const current = records.find((item) => item.id === selected);
  const configDirty = !!savedConfig && !equalConfig(config, savedConfig);
  const pendingApply =
    !!savedConfig &&
    !!appliedConfig &&
    !equalConfig(savedConfig, appliedConfig);
  // The event log is read again every few seconds while a run goes; the
  // record polls faster than that, so the parse waits for the log to change.
  const run = useMemo(() => parseRun(events), [events]);

  useEffect(() => {
    if (demo) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const response = await api("/environments");
        if (!response.ok) throw await failureOf(response);
        const next = ((await response.json()) as Environment[]).map(
          normalizeEnvironment,
        );
        if (active) setRecords(next);
      } catch (cause) {
        // The detail screen shows its own failures; a poll that missed one
        // round is not one of them.
        if (active) console.warn("Could not list virtual machines", cause);
      } finally {
        if (active) timer = setTimeout(poll, 1500);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [demo]);

  /*
   * A machine's own account of itself: the kernel and systemd coming up. It is
   * read out of the guest, so it answers once the guest is far enough along to
   * be asked.
   */
  useEffect(() => {
    if (demo || !selected) {
      setLogs("");
      return;
    }
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const running = current?.operation?.status === "running";
    const path = `/environments/${selected}/logs/boot`;
    const poll = async () => {
      try {
        const response = await api(path);
        if (!active) return;
        // A machine that is not running has no log of its own to give, and
        // saying so beats an empty pane that looks like a stall.
        setLogs(
          response.ok
            ? await response.text()
            : (await failureOf(response)).error,
        );
      } catch {
        if (active) setLogs("");
      } finally {
        if (active && running) timer = setTimeout(poll, 3000);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [demo, selected, current?.operation?.status]);

  /*
   * The Host's own account of the machine: every line each lifecycle action
   * printed, with the step markers the run panel is drawn from. It is read
   * again while an action runs and once more when it ends, so the last step
   * and the closing line are not missed.
   */
  useEffect(() => {
    if (!selected) {
      setEvents("");
      return;
    }
    if (demo) {
      setEvents(demoEvents[selected] ?? "");
      return;
    }
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const running = current?.operation?.status === "running";
    const path = `/environments/${encodeURIComponent(selected)}/events`;
    const poll = async () => {
      try {
        const response = await api(path);
        if (!active) return;
        setEvents(response.ok ? await response.text() : "");
      } catch {
        if (active) setEvents("");
      } finally {
        if (active && running) timer = setTimeout(poll, 3000);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [demo, selected, current?.operation?.status]);

  /*
   * Where the Operator lands. A machine with something happening to it opens on
   * its run, and a create that starts while they are reading something else
   * takes them there once, the way an image opens on its build log.
   * Every move after that is theirs to keep.
   */
  useEffect(() => {
    if (!selected) return;
    const running = current?.operation?.status === "running";
    if (opened.current !== selected) {
      opened.current = selected;
      wasRunning.current = running;
      setTab(running ? "run" : "summary");
      return;
    }
    if (running && !wasRunning.current) setTab("run");
    wasRunning.current = running;
  }, [selected, current?.operation?.status]);

  useEffect(() => {
    if (mode === "new") {
      setConfig(freshConfig());
      setSavedConfig(null);
      setAppliedConfig(null);
      setFailure(null);
      requestId.current = null;
      editorInitialized.current = null;
      return;
    }
    if (!selected) return;
    const found = records.find((item) => item.id === selected);
    if (found) {
      setConfig(structuredClone(found.config));
      setSavedConfig(structuredClone(found.config));
      setAppliedConfig(structuredClone(found.applied_config ?? found.config));
      setFailure(null);
      editorInitialized.current = selected;
    }
    if (demo) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const response = await api(
          `/environments/${encodeURIComponent(selected)}`,
        );
        if (!response.ok) throw await failureOf(response);
        const next = normalizeEnvironment(
          (await response.json()) as Environment,
        );
        if (active) {
          setRecords((items) => [
            next,
            ...items.filter((item) => item.id !== next.id),
          ]);
          if (next.applied_config)
            setAppliedConfig(structuredClone(next.applied_config));
          if (editorInitialized.current !== selected) {
            setConfig(structuredClone(next.config));
            setSavedConfig(structuredClone(next.config));
            setAppliedConfig(
              structuredClone(next.applied_config ?? next.config),
            );
            editorInitialized.current = selected;
          }
        }
      } catch (cause) {
        if (active) setFailure(asReport(cause));
      } finally {
        if (active) timer = setTimeout(poll, 1500);
      }
    };
    void poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
    // selected changes identify a new editor. Record polling must preserve edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, selected]);

  // A durable operation may have started in another tab or before this page
  // loaded. Keep every editor action disabled while it is running.
  useEffect(() => {
    setBusy(current?.operation?.status === "running");
  }, [current?.operation?.status]);

  function patch(change: Partial<EnvironmentConfig>) {
    setConfig((value) => ({ ...value, ...change }));
  }
  function patchRecipe(change: Partial<EnvironmentConfig["recipe"]>) {
    setConfig((value) => ({
      ...value,
      recipe: { ...value.recipe, ...change },
    }));
  }
  function chooseTemplate(id: string) {
    const template = customImageTemplate(id);
    if (!template) {
      patchRecipe({
        name: "",
        template_id: null,
        dependencies: [],
        setup: [],
        build_checks: [],
      });
      patch({ command: "", web_port: 0 });
      return;
    }
    patchRecipe({
      name: template.name,
      template_id: id,
      dependencies: structuredClone(template.dependencies),
      setup: structuredClone(template.setup),
      build_checks: structuredClone(template.buildChecks),
    });
    // A template brings its own service. The machine keeps its state under the
    // dev user's home rather than the data volume a container would have.
    patch({
      command: template.application.command.replace(
        "/data/t3home",
        "/home/dev/.local/state/t3",
      ),
      web_port: template.application.web_port,
    });
  }
  async function submitCreate(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setFailure(null);
    if (!requestId.current)
      requestId.current =
        globalThis.crypto?.randomUUID?.() ?? `create-${Date.now()}`;
    try {
      if (demo) {
        const record: Environment = {
          id: `demo-${Date.now()}`,
          config: structuredClone(config),
          state: "creating",
          service_ready: false,
          operation: {
            action: "create",
            status: "running",
            step: "Creating virtual machine",
          },
          log: "Creating virtual machine...",
          ssh_command: "ssh operator@host -p 2225",
          tunnel_command: "ssh -N -L 3003:127.0.0.1:3000 operator@host -p 2225",
          web_url: null,
          installed_versions: {},
        };
        setRecords((items) => [record, ...items]);
        onOpen(record.id);
        setTimeout(
          () =>
            setRecords((items) =>
              items.map((item) =>
                item.id === record.id
                  ? {
                      ...item,
                      state: "running",
                      service_ready: true,
                      operation: null,
                      log: `${item.log}\nSSH is ready\nService is responding`,
                    }
                  : item,
              ),
            ),
          900,
        );
      } else {
        const response = await api("/environments", {
          method: "POST",
          body: JSON.stringify({ request_id: requestId.current, config }),
        });
        if (!response.ok) throw await failureOf(response);
        const record = normalizeEnvironment(
          (await response.json()) as Environment,
        );
        setRecords((items) => [
          record,
          ...items.filter((item) => item.id !== record.id),
        ]);
        onOpen(record.id);
      }
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setBusy(false);
    }
  }
  async function saveConfig() {
    if (!current || busy || actionBusy(current)) return;
    setBusy(true);
    setFailure(null);
    try {
      if (!demo) {
        const response = await api(
          `/environments/${encodeURIComponent(current.id)}`,
          { method: "PUT", body: JSON.stringify(config) },
        );
        if (!response.ok) throw await failureOf(response);
      }
      setRecords((items) =>
        items.map((item) =>
          item.id === current.id
            ? { ...item, config: structuredClone(config) }
            : item,
        ),
      );
      setSavedConfig(structuredClone(config));
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setBusy(false);
    }
  }
  async function perform(action: string, environment = current, name?: string) {
    if (action === "apply_update" && configDirty) {
      setFailure({
        error: "Save configuration before applying the update.",
        caused_by: [],
      });
      return;
    }
    if (!environment || busy || actionBusy(environment)) return;
    setBusy(true);
    setFailure(null);
    try {
      if (demo) {
        setRecords((items) =>
          items.map((item) =>
            item.id !== environment.id
              ? item
              : {
                  ...item,
                  operation: {
                    action,
                    status: "running",
                    step:
                      action === "delete"
                        ? "Removing owned disk"
                        : `Running ${action}`,
                  },
                  log: `${item.log}\nStarting ${action}...`,
                },
          ),
        );
        setTimeout(
          () =>
            setRecords((items) =>
              items.flatMap((item) =>
                item.id !== environment.id
                  ? [item]
                  : action === "delete"
                    ? []
                    : [
                        {
                          ...item,
                          state: action === "stop" ? "stopped" : "running",
                          service_ready:
                            action === "start" ||
                            action === "restart" ||
                            action === "retry" ||
                            action === "bootstrap" ||
                            action === "apply_update",
                          operation: null,
                          log: `${item.log}\n${action} complete`,
                        },
                      ],
              ),
            ),
          900,
        );
      } else {
        const response = await api(
          `/environments/${encodeURIComponent(environment.id)}/actions`,
          {
            method: "POST",
            body: JSON.stringify({
              action,
              ...(name ? { confirm_name: name } : {}),
            }),
          },
        );
        if (!response.ok) throw await failureOf(response);
        const next = normalizeEnvironment(
          (await response.json()) as Environment,
        );
        setRecords((items) =>
          action === "delete" && next.state === "deleted"
            ? items.filter((item) => item.id !== environment.id)
            : items.map((item) => (item.id === environment.id ? next : item)),
        );
      }
      if (action === "delete") {
        setConfirming(null);
        setConfirmName("");
        onOpen(null);
      }
      if (action === "apply_update")
        setAppliedConfig(structuredClone(savedConfig ?? config));
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setBusy(false);
    }
  }

  // The same shape as a new Application: the screen says what it is, and the
  // form sits in a card, so the footer has an edge to reach.
  if (mode === "new")
    return (
      <>
        <PageHeader>
          <PageHeaderTitle>New virtual machine</PageHeaderTitle>
          <PageHeaderDescription>
            A workspace of your own on this Host, with the tools you pick and a
            service you can reach by name.
          </PageHeaderDescription>
        </PageHeader>
        <Card className="form-page">
          <EnvironmentEditor
            config={config}
            failure={failure}
            busy={busy}
            onChange={patch}
            onRecipeChange={patchRecipe}
            onTemplate={chooseTemplate}
            onCancel={() => onOpen(null)}
            onSubmit={submitCreate}
          />
        </Card>
      </>
    );
  if (!current)
    return (
      <Card>
        <EmptyState>
          <EmptyStateTitle>Virtual machine unavailable</EmptyStateTitle>
          <EmptyStateActions>
            <Button onClick={() => onOpen(null)}>Back to the Overview</Button>
          </EmptyStateActions>
        </EmptyState>
      </Card>
    );
  const operation = current.operation;
  const running = operation?.status === "running";
  // A half-provisioned guest reports numbers that read as a machine sitting
  // idle, so the glance waits for the service to answer. What it is using is
  // measured inside it: the Host only sees one hypervisor process.
  const samples = current.service_ready ? machineSeriesFor(metrics, current.id) : null;
  return (
    <>
      <div className="environment-page">
        {demo ? (
          <p className="demo-banner" role="status">
            <strong>Demo mode:</strong> memory-only state transitions. Live API
            actions are disabled.
          </p>
        ) : null}
        {failure ? <Failure failure={failure} /> : null}
        {operation?.error ? (
          <Failure
            failure={operation.error}
            actionLabel={
              operation.status === "failed" ? "Retry operation" : undefined
            }
            onAction={() => void perform("retry")}
          />
        ) : null}
        {/* The machine says its own name now, the way every other detail
            screen does. The breadcrumb repeating it is what a breadcrumb is
            for; a screen with no heading was the odd one out. */}
        <div className="between">
          <PageHeader>
            <PageHeaderTitle>{current.config.name}</PageHeaderTitle>
            {/* Where the machine is on the LAN, the way an Application's
                screen shows its Hostnames: the name, linked to the service
                when there is one, the address its lease gave it, and the MAC
                a reservation on the router is keyed on. */}
            <PageHeaderDescription>
              {current.web_url ? (
                <a href={current.web_url} target="_blank" rel="noreferrer" className="mono">
                  {current.web_url.replace(/^https?:\/\//, "")}
                </a>
              ) : (
                <span className="mono">{current.hostname || "No name"}</span>
              )}
              {" · "}
              {current.lan_address ? <span className="mono">{current.lan_address}</span> : "No lease"}
              {current.mac_address ? (
                <>
                  {" · "}
                  <span className="mono">{current.mac_address}</span>
                </>
              ) : null}
            </PageHeaderDescription>
            {samples ? <Glance samples={samples} /> : null}
          </PageHeader>
          {/* Neither badge moves during a run, so a third one says what is
              happening and how far along it is. The steps themselves walk in
              the Last run tab, which is where a run opens. */}
          <Lifecycle
            status={
              <>
                <StatusBadge tone={tone(current.state)}>
                  VM {current.state}
                </StatusBadge>
                {/* A machine nobody signed into has a T3 service that is off,
                    which is the normal state and not an error to report. One
                    still being created has no service yet to call disabled,
                    so the reading is not drawn until the create is over. */}
                {running && operation.action === "create" && !current.service_ready ? null : (
                  <StatusBadge tone={current.service_ready ? "success" : "neutral"}>
                    Service {current.service_ready ? "ready" : "disabled"}
                  </StatusBadge>
                )}
                {/* The one reading that says the Host is at work: the phase
                    the run is in, pulsing. The run tab has the steps. */}
                {running ? (
                  <StatusBadge tone="success" pulse>
                    {phase(operation.action, operation.step)}
                  </StatusBadge>
                ) : null}
              </>
            }
            actions={
              <>
                <Button
                  size="sm"
                  onClick={() => void perform("start")}
                  disabled={busy || actionBusy(current) || current.state === "running"}
                >
                  Start
                </Button>
                <Button
                  size="sm"
                  onClick={() => void perform("stop")}
                  disabled={busy || actionBusy(current) || current.state !== "running"}
                >
                  Stop
                </Button>
                <Button
                  size="sm"
                  onClick={() => void perform("restart")}
                  disabled={busy || actionBusy(current) || current.state !== "running"}
                >
                  Restart
                </Button>
                <Button
                  size="sm"
                  onClick={() => void perform("bootstrap")}
                  disabled={busy || actionBusy(current)}
                >
                  Bootstrap
                </Button>
              </>
            }
            destructive={
              <Button
                size="sm"
                variant="ghost"
                className="btn-danger-ghost"
                onClick={() => setConfirming(current)}
                disabled={busy || actionBusy(current)}
              >
                Delete virtual machine
              </Button>
            }
          />
        </div>
        <Card className="detail-tabs">
          <Tabs key={current.id} value={tab} onValueChange={setTab}>
            <TabsList aria-label="Virtual machine details">
              <TabsTrigger value="summary">Summary</TabsTrigger>
              <TabsTrigger value="configuration">Configuration</TabsTrigger>
              <TabsTrigger value="run">Last run</TabsTrigger>
              <TabsTrigger value="logs">Logs</TabsTrigger>
              <TabsTrigger value="connect">Terminal</TabsTrigger>
            </TabsList>
            <TabsContent value="summary">
              <Summary
                environment={current}
                saved={savedConfig ?? current.config}
                applied={appliedConfig ?? current.config}
                dirty={configDirty}
                pendingApply={pendingApply}
                run={run}
                busy={busy || actionBusy(current)}
                onOpen={setTab}
                onApply={() => void perform("apply_update")}
                onRetry={() => void perform("retry")}
              />
            </TabsContent>
            <TabsContent value="configuration">
              <EnvironmentEditor
                config={config}
                failure={null}
                busy={busy}
                onChange={patch}
                onRecipeChange={patchRecipe}
                onTemplate={chooseTemplate}
                onCancel={() => onOpen(null)}
                onSubmit={(event) => {
                  event.preventDefault();
                  void saveConfig();
                }}
                onDiscard={() => setConfig(structuredClone(savedConfig ?? current.config))}
                onApplyUpdate={() => void perform("apply_update")}
                onRetry={() => void perform("retry")}
                onOpenRun={() => setTab("run")}
                existing
                dirty={configDirty}
                pendingApply={pendingApply}
                failedAt={
                  operation?.status === "failed" && operation.step
                    ? stepLabel(operation.step)
                    : null
                }
              />
            </TabsContent>
            <TabsContent value="run" className="detail-run">
              <LastRun
                run={run}
                operation={operation}
                busy={busy || actionBusy(current)}
                onRetry={() => void perform("retry")}
                onOpenLog={() => setTab("logs")}
              />
            </TabsContent>
            <TabsContent value="logs" className="detail-logs">
            <LogSurface label="Virtual machine log" text={logs}
              placeholder="Nothing recorded yet." />
            </TabsContent>
            <TabsContent value="connect" className="detail-terminal">
            <Terminal id={current.id} machine />
            </TabsContent>
          </Tabs>
        </Card>
      </div>
      <AlertDialog
        open={!!confirming}
        onOpenChange={(open) => {
          if (!open) {
            setConfirming(null);
            setConfirmName("");
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete virtual machine</AlertDialogTitle>
          </AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              Deletion removes this virtual machine's disk, projects, credentials,
              and service state. Type <code>{confirming?.config.name}</code> to
              continue.
            </AlertDialogDescription>
            <Label htmlFor="confirm-environment-name">Virtual machine name</Label>
            <input
              id="confirm-environment-name"
              className="input"
              value={confirmName}
              onChange={(event) => setConfirmName(event.target.value)}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="btn-danger"
              disabled={confirmName !== confirming?.config.name || busy}
              onClick={() => {
                if (confirming) void perform("delete", confirming, confirmName);
              }}
            >
              Delete virtual machine
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/**
 * What the machine is, read without opening the form: whether what it runs
 * is what was saved, which tools it was asked for against which it has, how
 * its last run went, and what it serves. The versions used to sit under the
 * five-step form, past the Save, where nobody scrolled to find them.
 */
function Summary({
  environment,
  saved,
  applied,
  dirty,
  pendingApply,
  run,
  busy,
  onOpen,
  onApply,
  onRetry,
}: {
  environment: Environment;
  saved: EnvironmentConfig;
  applied: EnvironmentConfig;
  dirty: boolean;
  pendingApply: boolean;
  run: Run | null;
  busy: boolean;
  onOpen: (tab: string) => void;
  onApply: () => void;
  onRetry: () => void;
}) {
  const operation = environment.operation;
  const failed = operation?.status === "failed";
  const running = operation?.status === "running";
  const installed = Object.fromEntries(
    installedVersions(environment.installed_versions),
  );
  const appliedTools = new Set(applied.recipe.dependencies.map((d) => d.tool));
  const tools = saved.recipe.dependencies.map((dependency) => ({
    ...dependency,
    installed: installed[dependency.tool] as string | undefined,
    state: installed[dependency.tool]
      ? ("installed" as const)
      : appliedTools.has(dependency.tool)
        ? ("missing" as const)
        : ("pending" as const),
  }));
  const steps = run ? stepViews(run) : [];
  const stopped = steps.find((step) => step.state === "failed");
  const place = stopped ? position(run!.action, stopped.name) : null;
  return (
    <div className="summary">
      <section>
        <p className="t-caps">Configuration</p>
        {dirty ? (
          <p className="summary-state">
            <Badge variant="warning">Unsaved edits</Badge>
            <span>The configuration has changes that are not saved.</span>
            <Button size="sm" onClick={() => onOpen("configuration")}>
              Open configuration
            </Button>
          </p>
        ) : failed ? (
          <p className="summary-state">
            <Badge variant="danger">{NOUNS[operation.action] ?? operation.action} failed</Badge>
            <span>
              {pendingApply
                ? "The saved configuration could not be applied. The machine still runs the previous one."
                : "The machine may not have everything the configuration asks for."}
            </span>
            <Button size="sm" onClick={() => onOpen("run")}>
              Open last run
            </Button>
            <Button size="sm" variant="primary" onClick={onRetry} disabled={busy}>
              Retry {(NOUNS[operation.action] ?? operation.action).toLowerCase()}
            </Button>
          </p>
        ) : pendingApply ? (
          <p className="summary-state">
            <Badge variant="warning">Saved, not applied</Badge>
            <span>The machine still runs the previous configuration.</span>
            <Button size="sm" variant="primary" onClick={onApply} disabled={busy}>
              Apply update
            </Button>
          </p>
        ) : (
          <p className="summary-state">
            <Badge variant="success">Applied</Badge>
            <span>The machine runs the saved configuration.</span>
          </p>
        )}
      </section>
      <section>
        <p className="t-caps">Tools</p>
        {tools.length ? (
          <div className="table-wrap">
            <div className="table-scroll">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Tool</TableHead>
                    <TableHead>Requested</TableHead>
                    <TableHead>Installed</TableHead>
                    <TableHead />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {tools.map((tool) => (
                    <TableRow key={tool.tool}>
                      <TableCell>
                        <code>{tool.tool}</code>
                      </TableCell>
                      <TableCell className="mono">
                        {tool.version}
                        {tool.allow_builds?.length ? (
                          <span className="muted"> allow_builds={tool.allow_builds.join(",")}</span>
                        ) : null}
                      </TableCell>
                      <TableCell className="mono">{tool.installed ?? "—"}</TableCell>
                      <TableCell>
                        {tool.state === "installed" ? (
                          <Badge variant="success">Installed</Badge>
                        ) : tool.state === "missing" ? (
                          <Badge variant="danger">Missing</Badge>
                        ) : (
                          <Badge variant="warning">Not applied yet</Badge>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </div>
        ) : (
          <p className="muted">No tools in the recipe. This is a plain Ubuntu.</p>
        )}
      </section>
      <section>
        <p className="t-caps">Last run</p>
        {run ? (
          <>
          <p className="summary-state">
            {running ? (
              <StatusBadge tone="success" pulse>Running</StatusBadge>
            ) : run.status === "failed" ? (
              <StatusBadge tone="danger">Failed</StatusBadge>
            ) : (
              <StatusBadge tone="success">Succeeded</StatusBadge>
            )}
            <span>
              {running
                ? TITLES[run.action] ?? run.action
                : NOUNS[run.action] ?? run.action}
              {run.status === "failed" && stopped
                ? ` · stopped at ${place ? `step ${place.at} of ${place.of}, ` : ""}${stepLabel(stopped.name)}`
                : run.endedAt
                  ? ` · ${formatSeconds(seconds(run.startedAt, run.endedAt))}`
                  : ""}
            </span>
            <Button size="sm" onClick={() => onOpen("run")}>
              Open last run
            </Button>
          </p>
          {/* One segment per step while it goes or after it failed; a run
              that ended well is the badge alone. */}
          {running || run.status === "failed" ? (
            <StepBar
              className="summary-bar"
              label={running ? TITLES[run.action] ?? run.action : NOUNS[run.action] ?? run.action}
              steps={steps.map((step) => ({ key: step.name, label: stepLabel(step.name), state: step.state }))}
            />
          ) : null}
          </>
        ) : (
          <p className="muted">Nothing has run on this machine yet.</p>
        )}
      </section>
      <section>
        <p className="t-caps">Service</p>
        <dl className="summary-facts">
          <dt>Command</dt>
          <dd className="mono">{applied.command || "None"}</dd>
          <dt>Web port</dt>
          <dd className="mono">{applied.web_port || "None"}</dd>
          <dt>Machine</dt>
          <dd>
            {applied.cpus} CPUs · {applied.memory_gib} GiB memory · {applied.disk_gib} GiB disk
          </dd>
        </dl>
      </section>
    </div>
  );
}

export function EnvironmentEditor({
  config,
  failure,
  busy,
  onChange,
  onRecipeChange,
  onTemplate,
  onCancel,
  onSubmit,
  onDiscard,
  onApplyUpdate,
  onRetry,
  onOpenRun,
  existing = false,
  dirty = false,
  pendingApply = false,
  failedAt = null,
}: {
  config: EnvironmentConfig;
  failure: Report | null;
  busy: boolean;
  onChange: (change: Partial<EnvironmentConfig>) => void;
  onRecipeChange: (change: Partial<EnvironmentConfig["recipe"]>) => void;
  onTemplate: (id: string) => void;
  onCancel: () => void;
  onSubmit: (event: FormEvent) => void;
  onDiscard?: () => void;
  onApplyUpdate?: () => void;
  onRetry?: () => void;
  onOpenRun?: () => void;
  existing?: boolean;
  dirty?: boolean;
  pendingApply?: boolean;
  /** The step the last run stopped at, when it failed. */
  failedAt?: string | null;
}) {
  // A machine needs a name and a size. Everything else is an offer: no
  // template, no tools and no service is a plain Ubuntu, which is a thing an
  // Operator may well want.
  const valid =
    config.name.trim().length > 0 &&
    config.cpus > 0 &&
    config.memory_gib > 0 &&
    config.disk_gib > 0 &&
    config.recipe.dependencies.every((dependency) =>
      isVersion(dependency.version),
    );
  return (
    <Form className="environment-editor" onSubmit={onSubmit}>
      <div className="form-body">
      <Steps>
        <Step title="Machine">
          <div className="field-row">
            <FormField
              id="environment-name"
              label="Name"
              value={config.name}
              onChange={(event) => onChange({ name: event.target.value })}
              required
              disabled={busy}
            />
            <FormField id="environment-template" label="Template">
              <select
                id="environment-template"
                className="input"
                value={config.recipe.template_id ?? ""}
                onChange={(event) => onTemplate(event.target.value)}
                disabled={busy}
              >
                <option value="">None</option>
                {customImageTemplates.map((template) => (
                  <option key={template.id} value={template.id}>
                    {template.name}
                  </option>
                ))}
              </select>
            </FormField>
          </div>
          <div className="field-row">
            <FormField
              id="environment-cpus"
              label="CPUs"
              type="number"
              min={1}
              max={64}
              value={String(config.cpus)}
              onChange={(event) => onChange({ cpus: Number(event.target.value) })}
              disabled={busy || existing}
              required
            />
            <FormField
              id="environment-memory"
              label="Memory (GiB)"
              type="number"
              min={1}
              max={512}
              value={String(config.memory_gib)}
              onChange={(event) =>
                onChange({ memory_gib: Number(event.target.value) })
              }
              disabled={busy || existing}
              required
            />
            <FormField
              id="environment-disk"
              label="Disk (GiB)"
              type="number"
              min={8}
              max={4096}
              value={String(config.disk_gib)}
              onChange={(event) =>
                onChange({ disk_gib: Number(event.target.value) })
              }
              disabled={busy || existing}
              required
            />
          </div>
          <FormField id="environment-os" label="Base OS">
            <select
              id="environment-os"
              className="input"
              value="ubuntu-lts"
              disabled
            >
              <option value="ubuntu-lts">Ubuntu LTS</option>
            </select>
          </FormField>
        </Step>
        <Step title="Mise packages and dependencies">
          <Dependencies
            id="environment-dependencies"
            value={config.recipe.dependencies}
            onChange={(dependencies) => onRecipeChange({ dependencies })}
            disabled={busy}
          />
        </Step>
        <Step title="Custom commands">
          <FormField
            id="environment-setup"
            label="Custom commands"
            hint="One command per line. Runs as root, with network, after the dependencies. This is where a shell installer or an apt package goes."
          >
            <ShellEditor
              id="environment-setup"
              value={config.recipe.setup.join("\n")}
              onChange={(value) =>
                onRecipeChange({
                  setup: value
                    .split("\n")
                    .map((line) => line.trim())
                    .filter(Boolean),
                })
              }
              disabled={busy}
            />
          </FormField>
        </Step>
        <Step title="Build checks">
          <FormField
            id="environment-checks"
            label="Build checks"
            hint="One verification command per line."
          >
            <ShellEditor
              id="environment-checks"
              value={config.recipe.build_checks.join("\n")}
              onChange={(value) =>
                onRecipeChange({
                  build_checks: value
                    .split("\n")
                    .map((line) => line.trim())
                    .filter(Boolean),
                })
              }
              disabled={busy}
            />
          </FormField>
        </Step>
        <Step title="Service">
          <div className="field-row">
            <FormField
              id="environment-command"
              label="Service command"
              value={config.command}
              onChange={(event) => onChange({ command: event.target.value })}
              disabled={busy}
            />
            <FormField
              id="environment-port"
              label="Web port"
              type="number"
              min={1}
              max={65535}
              value={config.web_port ? String(config.web_port) : ""}
              placeholder="None"
              onChange={(event) =>
                onChange({ web_port: Number(event.target.value) || 0 })
              }
              disabled={busy}
            />
          </div>
        </Step>
      </Steps>
      {/* The console opens its own terminal, so a key is only for reaching
          the machine from somewhere else. Out of the way, not out of reach. */}
      <details className="advanced">
        <summary>Advanced</summary>
        <FormField
          id="environment-ssh-key"
          label="SSH public key"
          hint="Optional. Lets you reach the machine with your own ssh client. The private key never leaves your computer."
        >
          <textarea
            id="environment-ssh-key"
            className="input"
            rows={3}
            value={config.ssh_public_key}
            onChange={(event) =>
              onChange({ ssh_public_key: event.target.value })
            }
            disabled={busy}
          />
        </FormField>
      </details>
      {failure ? <Failure failure={failure} /> : null}
      </div>
      {/* The footer says where the configuration stands and offers the one
          move that follows: save what changed, apply what was saved, or retry
          what failed. It sticks to the bottom of the panel, so the state is
          read without scrolling past five steps to find it. */}
      {!existing ? (
        <FormActions sticky>
          <Button type="button" size="sm" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" size="sm" variant="primary" disabled={busy || !valid}>
            Create virtual machine
          </Button>
        </FormActions>
      ) : dirty ? (
        <FormActions
          sticky
          tone="warning"
          message={
            <>
              <strong>Unsaved changes.</strong> The machine keeps running as it is until you save and apply.
            </>
          }
        >
          <Button type="button" size="sm" onClick={onDiscard} disabled={busy}>
            Discard
          </Button>
          <Button type="submit" size="sm" variant="primary" disabled={busy || !valid}>
            Save configuration
          </Button>
        </FormActions>
      ) : failedAt ? (
        <FormActions
          sticky
          tone="danger"
          message={
            <>
              <strong>The last run failed</strong> at {failedAt}.{" "}
              {pendingApply
                ? "The machine still runs the previous configuration."
                : "The machine may not have everything this configuration asks for."}
            </>
          }
        >
          <Button type="button" size="sm" onClick={onOpenRun}>
            Open last run
          </Button>
          <Button type="button" size="sm" variant="primary" onClick={onRetry} disabled={busy}>
            Retry
          </Button>
        </FormActions>
      ) : pendingApply ? (
        /* Saved and not yet on the machine is the same warning the Summary
           shows for it: the machine is not running what is written here. */
        <FormActions
          sticky
          tone="warning"
          message={
            <>
              <strong>Saved, not applied.</strong> The machine still runs the previous configuration.
            </>
          }
        >
          <Button type="button" size="sm" variant="primary" onClick={onApplyUpdate} disabled={busy}>
            Apply update
          </Button>
        </FormActions>
      ) : (
        <FormActions sticky message="Saved and applied." />
      )}
    </Form>
  );
}
