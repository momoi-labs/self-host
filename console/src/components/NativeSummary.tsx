import type { App } from "../lib/types.js";
import { formatBytes } from "../lib/format.js";

export function NativeSummary({ app }: { app: App }) {
  if (app.runtime?.kind !== "native") return null;
  const native = app.runtime;
  return <>
    <dl className="summary-facts">
      <dt>Runtime</dt><dd>Native process</dd>
      <dt>Application Account</dt><dd><code>{native.account}</code></dd>
      <dt>Process readiness</dt><dd>{app.services?.[0]?.health === "healthy" ? "Ready" : app.status === "stopped" ? "Stopped" : "Not ready"}</dd>
      <dt>Program</dt><dd><code>{native.command[0]}</code></dd>
      <dt>Working directory</dt><dd>{native.working_dir || "Application home"}</dd>
      <dt>Configured dependencies</dt><dd>{native.recipe?.dependencies.length ? <ul>{native.recipe.dependencies.map((dependency) => <li key={dependency.tool}>
        <code>{dependency.tool}@{dependency.version}</code>
        {dependency.allow_builds?.length ? <> <code>allow_builds={dependency.allow_builds.join(", ")}</code></> : null}
      </li>)}</ul> : "None"}</dd>
      <dt>Configured setup</dt><dd>{native.recipe?.setup.length ? `${native.recipe.setup.length} ${native.recipe.setup.length === 1 ? "command" : "commands"}` : "None"}</dd>
      <dt>Publication</dt><dd>{app.publication?.kind === "unpublished" ? "No HTTP route" : `Loopback port ${native.port}`}</dd>
      <dt>CPU limit</dt><dd>{native.limits?.cpu_percent ? `${native.limits.cpu_percent}%` : "Unlimited"}</dd>
      <dt>Memory limit</dt><dd>{native.limits?.memory_bytes ? formatBytes(native.limits.memory_bytes) : "Unlimited"}</dd>
      <dt>Task limit</dt><dd>{native.limits?.max_tasks ?? "Unlimited"}</dd>
    </dl>
  </>;
}
