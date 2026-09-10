import { useEffect, useRef, useState, type FormEvent } from "react";
import {
  Button, Card, FormField, Pane, Split, Splitter,
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle, AlertDialogDescription,
  AlertDialogFooter, AlertDialogCancel, AlertDialogAction,
  EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateIcon, EmptyStateTitle,
  PageHeader, PageHeaderDescription, PageHeaderTitle,
  LogView, LogViewLine, Search,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { Failure } from "../components/Failure.js";
import { Icon } from "../components/Icon.js";
import { ShellEditor } from "../components/ShellEditor.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { api, asReport, failureOf } from "../lib/api.js";
import type { Report } from "../lib/types.js";
import { devImageTemplate, devImageTemplates, type ImageDependency } from "../lib/devImageTemplates.js";

type Dependency = ImageDependency;
type DevImage = {
  id: string;
  name: string;
  template_id?: string | null;
  dependencies: Dependency[];
  build_checks?: string[];
  image: string;
  status: "building" | "ready" | "failed";
  last_error: Report | null;
  log: string;
  in_use?: boolean | null;
};

type MiseTool = { name: string; description?: string; backends: string[] };

let toolCatalog: Promise<MiseTool[]> | null = null;

function loadToolCatalog(): Promise<MiseTool[]> {
  if (!toolCatalog) toolCatalog = (async () => {
    const response = await api("/dev-images/tools");
    if (!response.ok) throw await failureOf(response);
    const payload: unknown = await response.json();
    if (!Array.isArray(payload)) throw new Error("Invalid tool search response");
    return payload.flatMap((entry: unknown): MiseTool[] => {
      if (typeof entry === "string") return [{ name: entry, backends: [] }];
      if (!entry || typeof entry !== "object" || !("name" in entry) || typeof entry.name !== "string") return [];
      return [{
        name: entry.name,
        backends: "backends" in entry && Array.isArray(entry.backends)
          ? entry.backends.filter((key): key is string => typeof key === "string") : [],
      }];
    });
  })().catch((error) => { toolCatalog = null; throw error; });
  return toolCatalog;
}

function Dependencies({ value, onChange, disabled }: {
  value: Dependency[];
  onChange: (value: Dependency[]) => void;
  disabled: boolean;
}) {
  const [tools, setTools] = useState<MiseTool[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchFailed, setSearchFailed] = useState(false);
  const [query, setQuery] = useState("");
  const [focused, setFocused] = useState(false);
  const [active, setActive] = useState(0);
  const [chosen, setChosen] = useState<string | null>(null);
  const [version, setVersion] = useState("latest");
  const search = useRef<HTMLInputElement>(null);
  const versionInput = useRef<HTMLInputElement>(null);
  const returnToSearch = useRef(false);
  const term = query.toLowerCase().trim();
  const matches = [...new Set(tools.flatMap((tool) => [tool.name, ...tool.backends.filter((key) => key.includes(":"))]))]
    .filter((tool) => tool.toLowerCase().includes(term) && !value.some((dep) => dep.tool === tool))
    .sort((a, b) => {
      const rank = (key: string) => key.toLowerCase() === term ? 0 : key.toLowerCase().startsWith(term) ? 1 : 2;
      return rank(a) - rank(b) || a.localeCompare(b);
    }).slice(0, 20);
  // A valid mise key can be entered even when the remote catalog is unavailable.
  const direct = /^[a-zA-Z0-9][a-zA-Z0-9._:/@-]{0,159}$/.test(query.trim()) ? query.trim() : null;
  if (direct && (direct.includes(":") || !matches.length) && !matches.includes(direct) && !value.some((dep) => dep.tool === direct)) matches.unshift(direct);
  const suggestion = focused && term ? matches[active % Math.max(matches.length, 1)] : undefined;

  useEffect(() => {
    let active = true;
    setLoading(true);
    void loadToolCatalog()
      .then((results) => { if (active) setTools(results); })
      .catch(() => { if (active) setSearchFailed(true); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (chosen) {
      versionInput.current?.focus();
      versionInput.current?.select();
    } else if (returnToSearch.current) {
      returnToSearch.current = false;
      search.current?.focus();
    }
  }, [chosen]);

  function choose(tool: string, selectedVersion = "latest") {
    setChosen(tool);
    setVersion(selectedVersion);
  }

  function finish() {
    returnToSearch.current = true;
    setChosen(null);
    setQuery("");
    setActive(0);
  }

  function add() {
    if (!chosen || !version.trim() || !versionInput.current?.reportValidity()) return;
    const dependency = { tool: chosen, version: version.trim() };
    onChange(value.some((dep) => dep.tool === chosen)
      ? value.map((dep) => dep.tool === chosen ? { ...dep, ...dependency } : dep)
      : [...value, dependency]);
    finish();
  }

  function editor(tool: string) {
    return (
      <span className="dependency-chip dependency-chip-draft" key={tool}>
        <label className="dependency-chip-key mono" htmlFor="dependency-version">{tool}</label>
        <input ref={versionInput} id="dependency-version" aria-label={`${tool} version`}
          className="dependency-version-input mono" value={version} maxLength={64} required disabled={disabled}
          style={{ width: `${Math.max(6, Math.min(version.length + 1, 18))}ch` }}
          pattern="[a-zA-Z0-9][a-zA-Z0-9._\-]*" onChange={(event) => setVersion(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || (event.key === "Tab" && !event.shiftKey)) { event.preventDefault(); add(); }
            if (event.key === "Escape") { event.preventDefault(); finish(); }
          }}
        />
        <button type="button" className="dependency-chip-remove" disabled={disabled} aria-label={`Confirm ${tool}`} onClick={add}>✓</button>
        <button type="button" className="dependency-chip-remove" disabled={disabled} aria-label={`Cancel ${tool} edit`} onClick={finish}>×</button>
      </span>
    );
  }

  return (
    <div className="stack">
      <div>
        <label className="t-label" htmlFor="dependency-search">Dependencies</label>
        <p className="muted t-label" id="dependency-help">Type a mise key or search. Tab or Enter selects a tool and confirms its version. Arrow keys cycle suggestions.</p>
      </div>
      <div className="dependency-chips" onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false);
      }}>
        {value.map((dep) => chosen === dep.tool ? editor(dep.tool) : (
          <span className="dependency-chip" key={dep.tool}>
            <button type="button" className="dependency-chip-edit mono" disabled={disabled}
              aria-label={`Edit ${dep.tool} version`} onClick={() => choose(dep.tool, dep.version)}>
              <span>{dep.tool}</span><span>{dep.version}</span>
            </button>
            <button type="button" className="dependency-chip-remove" disabled={disabled}
              aria-label={`Remove ${dep.tool}`} onClick={() => onChange(value.filter((item) => item.tool !== dep.tool))}>×</button>
          </span>
        ))}
        {chosen && !value.some((dep) => dep.tool === chosen) ? editor(chosen) : null}
        {!chosen ? (
          <span className="dependency-completion">
            <input ref={search} id="dependency-search" className="dependency-search mono"
              aria-autocomplete="inline" aria-describedby="dependency-help dependency-suggestion"
              style={{ width: `${Math.max(query ? 2 : 20, Math.min(query.length + 1, 40))}ch` }}
              placeholder="node, claude, npm:t3..." autoComplete="off"
              disabled={disabled} value={query}
              onFocus={() => setFocused(true)}
              onChange={(event) => { setQuery(event.target.value); setActive(0); setFocused(true); }}
              onKeyDown={(event) => {
                if (event.key === "Escape") { event.preventDefault(); setQuery(""); }
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  setActive((index) => (index + (event.key === "ArrowDown" ? 1 : -1) + matches.length) % Math.max(matches.length, 1));
                }
                if (event.key === "Enter" || (event.key === "Tab" && !event.shiftKey && suggestion)) {
                  event.preventDefault();
                  if (suggestion) choose(suggestion);
                }
              }}
            />
            <span id="dependency-suggestion" className="dependency-suggestion" aria-live="polite">
              {suggestion ? (
                <button type="button" className="dependency-inline-option mono" disabled={disabled}
                  aria-label={`Select ${suggestion}`} onMouseDown={(event) => event.preventDefault()} onClick={() => choose(suggestion)}>
                  <span>{suggestion}</span><kbd>Tab</kbd>
                  {matches.length > 1 ? <small>{active % matches.length + 1}/{matches.length}</small> : null}
                </button>
              ) : focused && term ? <span className="muted t-label">{loading ? "Searching mise..." : searchFailed ? "Catalog unavailable. Enter a mise key, e.g. just or npm:t3." : "No match. Try a backend key, e.g. npm:t3."}</span> : null}
              {focused && term && suggestion && (loading || searchFailed) ? (
                <small className="muted">{loading ? "Loading suggestions..." : "Catalog unavailable. Using the entered mise key."}</small>
              ) : null}
            </span>
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function DevImages({ listing, selected, onOpen }: {
  listing: boolean;
  selected: string | null;
  onOpen: (id: string | null) => void;
}) {
  const [images, setImages] = useState<DevImage[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [name, setName] = useState("");
  const [templateId, setTemplateId] = useState("");
  const [dependencies, setDependencies] = useState<Dependency[]>([]);
  const [buildChecks, setBuildChecks] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const [loadFailure, setLoadFailure] = useState<Report | null>(null);
  const [query, setQuery] = useState("");
  const [confirming, setConfirming] = useState<DevImage | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [editorKey, setEditorKey] = useState(0);
  const building = images.some((image) => image.status === "building");
  const current = images.find((image) => image.id === selected);
  const editingDisabled = submitting || current?.status === "building";
  const checks = buildChecks.split("\n").map((command) => command.trim()).filter(Boolean);
  const mise = `[tools]\n${dependencies.map(({ tool, version, allow_builds }) => `${JSON.stringify(tool)} = ${allow_builds?.length ? `{ version = ${JSON.stringify(version)}, allow_builds = ${JSON.stringify(allow_builds)} }` : JSON.stringify(version)}`).join("\n")}\n`
    + (checks.length ? `\n[tasks.check]\nrun = ${JSON.stringify(checks)}\n` : "");

  useEffect(() => {
    setName(current?.name ?? "");
    setTemplateId(current?.template_id ?? "");
    setDependencies(current?.dependencies ?? []);
    setBuildChecks((current?.build_checks ?? []).join("\n"));
    setFailure(null);
    setEditorKey((value) => value + 1);
    // Load the recipe when opening it; log polls must not overwrite edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listing, selected, current?.id]);

  function chooseTemplate(id: string) {
    const template = devImageTemplate(id);
    const previous = devImageTemplate(templateId);
    setTemplateId(id);
    setName((name) => !name || name === previous?.imageName ? template?.imageName ?? "" : name);
    setDependencies(structuredClone(template?.dependencies ?? []));
    setBuildChecks((template?.buildChecks ?? []).join("\n"));
    setEditorKey((value) => value + 1);
  }

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    async function poll() {
      try {
        const response = await api("/dev-images", { signal: controller.signal });
        if (!response.ok) throw await failureOf(response);
        const records = await response.json() as DevImage[];
        if (!active) return;
        setImages(records);
        setLoadFailure(null);
        setLoaded(true);
      } catch (cause) {
        if (active) setLoadFailure(asReport(cause));
      } finally {
        if (active) timer = setTimeout(poll, 1000);
      }
    }
    void poll();
    return () => { active = false; controller.abort(); clearTimeout(timer); };
  }, [refresh]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setFailure(null);
    try {
      const response = await api("/dev-images", {
        method: "POST", body: JSON.stringify({ id: selected, name: name.trim(), template_id: templateId || null, dependencies, build_checks: checks }),
      });
      if (!response.ok) throw await failureOf(response);
      const image = await response.json() as DevImage;
      setImages((records) => [image, ...records.filter((item) => item.id !== image.id)]);
      onOpen(image.id);
      setName(image.name);
      setRefresh((value) => value + 1);
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSubmitting(false);
    }
  }

  const visible = images.filter((image) => image.name.toLowerCase().includes(query.trim().toLowerCase()));

  async function remove(image: DevImage) {
    setDeleting(image.id);
    setFailure(null);
    try {
      const response = await api(`/dev-images/${encodeURIComponent(image.id)}`, { method: "DELETE" });
      if (!response.ok) throw await failureOf(response);
      setImages((images) => images.filter((item) => item.id !== image.id));
      setRefresh((value) => value + 1);
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setDeleting(null);
      setConfirming(null);
    }
  }

  function deleteReason(image: DevImage) {
    if (image.status === "building") return "Build in progress";
    if (image.in_use) return "In use";
    if (image.in_use !== false) return "Usage unavailable";
    return null;
  }

  if (listing) return (
    <>
      <PageHeader>
        <PageHeaderTitle>Development images</PageHeaderTitle>
        <PageHeaderDescription>Saved images and their latest build on this Host.</PageHeaderDescription>
      </PageHeader>
      {loadFailure ? <Failure failure={loadFailure} /> : null}
      {failure ? <Failure failure={failure} /> : null}
      <div className="dashboard-filters">
        <Search aria-label="Search by name" placeholder="Search by name..." value={query}
          onChange={(event) => setQuery(event.target.value)} />
        <Button size="sm" variant="ghost" onClick={() => setQuery("")}>Clear filters</Button>
        <Button size="sm" variant="primary" onClick={() => onOpen(null)}><Icon name="plus" />New image</Button>
      </div>
      {!loaded && !loadFailure ? <p className="muted">Loading images...</p> : loaded && !images.length ? (
        <Card>
          <EmptyState variant="first-run" className="hatch">
            <EmptyStateIcon><Icon name="box" size="lg" /></EmptyStateIcon>
            <EmptyStateTitle>No development images yet</EmptyStateTitle>
            <EmptyStateDescription>Choose your mise dependencies and build your first development image.</EmptyStateDescription>
            <EmptyStateActions><Button size="sm" variant="primary" onClick={() => onOpen(null)}><Icon name="plus" />New image</Button></EmptyStateActions>
          </EmptyState>
        </Card>
      ) : loaded ? (
        <div className="stack dev-image-list">
          <div className="table-wrap">
            <div className="table-scroll">
              <Table>
                <TableHeader><TableRow>
                  <TableHead scope="col">Name</TableHead>
                  <TableHead scope="col">Dependencies</TableHead>
                  <TableHead scope="col">Image</TableHead>
                  <TableHead scope="col">Status</TableHead>
                  <TableHead scope="col">Actions</TableHead>
                </TableRow></TableHeader>
                <TableBody>
                  {!visible.length ? <TableRow><TableCell colSpan={5} className="muted">No images match your filters.</TableCell></TableRow> : visible.map((image) => (
                    <TableRow key={image.id} tabIndex={0} role="button" aria-label={`Open ${image.name}`}
                      onClick={() => onOpen(image.id)} onKeyDown={(event) => {
                        if (event.key !== "Enter" && event.key !== " ") return;
                        event.preventDefault();
                        onOpen(image.id);
                      }}>
                      <TableCell>{image.name}</TableCell>
                      <TableCell className="mono">{image.dependencies.map((dep) => `${dep.tool}@${dep.version}`).join(", ")}</TableCell>
                      <TableCell className="mono">{image.image}</TableCell>
                      <TableCell><StatusBadge tone={image.status === "ready" ? "success" : image.status === "failed" ? "danger" : "neutral"}>
                        {image.status === "ready" ? "Built" : image.status === "failed" ? "Failed" : "Building"}
                      </StatusBadge></TableCell>
                      <TableCell onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
                        <Button type="button" size="sm" variant="ghost" className="btn-danger-ghost"
                          aria-label={`Delete ${image.name}`} disabled={!!deleting || !!deleteReason(image)}
                          onClick={() => setConfirming(image)}>
                          {deleting === image.id ? "Deleting..." : "Delete"}
                        </Button>
                        {deleteReason(image) ? <span className="muted t-label">{deleteReason(image)}</span> : null}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            <p className="table-footer">{visible.length} of {images.length} images</p>
          </div>
        </div>
      ) : null}
      <AlertDialog open={!!confirming} onOpenChange={(open) => { if (!open) setConfirming(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader><AlertDialogTitle>Delete image</AlertDialogTitle></AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>Delete <code>{confirming?.name}</code> and its local image tags?</AlertDialogDescription>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="btn-danger" onClick={() => { if (confirming) void remove(confirming); }}>Delete image</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );

  return (
    <div className="dev-images-page">
      <PageHeader>
        <PageHeaderTitle>{selected ? current?.name ?? "Image" : "New image"}</PageHeaderTitle>
        <PageHeaderDescription>Save your image's dependencies and build it on this Host.</PageHeaderDescription>
      </PageHeader>
      {loadFailure ? <Failure failure={loadFailure} /> : null}
      <Card className="dev-images-panel">
        <Split>
          <Pane>
            <form className="stack" onSubmit={submit}>
              <div className="row">
                <p className="t-caps grow">Configuration</p>
                {current ? <StatusBadge tone={current.status === "ready" ? "success" : current.status === "failed" ? "danger" : "neutral"}>
                  {current.status === "ready" ? "Built" : current.status === "failed" ? "Failed" : "Building"}
                </StatusBadge> : null}
              </div>
              {!selected ? (
                <FormField id="dev-image-template" label="Template"
                  hint="Start with a template, then edit any field before building.">
                  <select id="dev-image-template" className="input" value={templateId}
                    disabled={editingDisabled} onChange={(event) => chooseTemplate(event.target.value)}>
                    <option value="">Custom image</option>
                    {devImageTemplates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}
                  </select>
                </FormField>
              ) : null}
              <FormField id="dev-image-name" label="Image name" placeholder="web-dev"
                value={name} onChange={(event) => setName(event.target.value)}
                required maxLength={128} disabled={editingDisabled}
              />
              <Dependencies key={editorKey} value={dependencies} onChange={setDependencies} disabled={editingDisabled} />
              {dependencies.filter((dep) => dep.tool.startsWith("npm:")).map((dep, index) => (
                <FormField key={`${editorKey}-${dep.tool}`} id={`allow-builds-${index}`} label={`${dep.tool} allow_builds`}
                  defaultValue={(dep.allow_builds ?? []).join(", ")} disabled={editingDisabled}
                  hint="Package names whose install scripts may run, comma separated. Leave empty to use mise defaults."
                  onChange={(event) => {
                    const allow_builds = event.target.value.split(",").map((name) => name.trim()).filter(Boolean);
                    setDependencies((current) => current.map((item) => item.tool === dep.tool ? { ...item, allow_builds } : item));
                  }} />
              ))}
              <FormField id="build-checks" label="Build checks (optional)"
                hint="One command per line, saved as tasks.check.run. Runs as dev without network access, with a 60-second limit per command. Any failure stops the build.">
                <ShellEditor id="build-checks" value={buildChecks} onChange={setBuildChecks}
                  disabled={editingDisabled} maxLength={65536}
                  placeholder={"t3 --help\nclaude --version\ncodex --version"} />
              </FormField>
              <details className="disclosure">
                <summary>mise.toml</summary>
                <pre className="dev-image-code">{mise}</pre>
              </details>
              {failure ? <Failure failure={failure} /> : null}
              {current?.last_error ? <Failure failure={current.last_error} /> : null}
              <p className="muted t-label">{building ? "A build is running on this Host." : "Debian 13 with mise and Git. Save and build to apply your changes."}</p>
              <div className="form-actions">
                <Button type="submit" variant="primary" disabled={submitting || building || !loaded || !!loadFailure || !dependencies.length || (!!selected && !current)}>
                  {submitting ? "Saving..." : current?.status === "building" ? "Building..." : "Save and build"}
                </Button>
              </div>
            </form>
          </Pane>
          <Splitter defaultSize={48} min={30} max={65} aria-label="Resize the image editor and build log" />
          <Pane className="pane-logs dev-image-output">
            <p className="t-caps">Logs</p>
            <LogView key={current?.id ?? "new"} role="log" aria-live="polite" tabIndex={0} aria-label="Build logs">
              {(current?.log || "Save and build to see the output here.").split("\n").map((line, index) => (
                <LogViewLine key={index}>{line || " "}</LogViewLine>
              ))}
            </LogView>
          </Pane>
        </Split>
      </Card>
    </div>
  );
}
