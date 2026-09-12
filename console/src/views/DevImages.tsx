import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import {
  Badge, Button, Card, FormField, Label,
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle, AlertDialogDescription,
  AlertDialogFooter, AlertDialogCancel, AlertDialogAction,
  Chip, ChipInput, ChipInputBox, ChipInputEmpty, ChipInputField, ChipInputList, ChipInputOption,
  ChipName, ChipOption, ChipOptionAdd, ChipRemove, ChipScope, ChipValue,
  EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateIcon, EmptyStateTitle,
  PageHeader, PageHeaderDescription, PageHeaderTitle,
  LogView, LogViewLine, Search,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
  ValidationMessage,
} from "@momoi-labs/kiso-react";

import { Failure } from "../components/Failure.js";
import { Icon } from "../components/Icon.js";
import { ShellEditor } from "../components/ShellEditor.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { api, asReport, failureOf } from "../lib/api.js";
import type { Report } from "../lib/types.js";
import { devImageTemplate, devImageTemplates, type ImageDependency } from "../lib/devImageTemplates.js";
import { recipeBody } from "../lib/devImageRecipe.js";
import {
  ALLOW_BUILDS, isKey, isVersion, readOption, splitKey, suggest, takesAllowBuilds,
} from "../lib/dependencies.js";

type Dependency = ImageDependency;
type DevImage = {
  id: string;
  name: string;
  template_id?: string | null;
  dependencies: Dependency[];
  setup?: string[];
  build_checks?: string[];
  dockerfile?: string | null;
  image: string;
  status: "building" | "ready" | "failed";
  last_error: Report | null;
  log: string;
  in_use?: boolean | null;
};

type MiseTool = { name: string; description?: string; backends: string[] };

/** How much of the card the build log takes: none, the lower half, or all. */
type Dock = "closed" | "half" | "full";

function statusTone(image: DevImage) {
  return image.status === "ready" ? "success" : image.status === "failed" ? "danger" : "neutral";
}

function statusLabel(image: DevImage) {
  return image.status === "ready" ? "Built" : image.status === "failed" ? "Failed" : "Building";
}

/**
 * One numbered section of the form. The four of them run in the order the
 * Dockerfile does, so the Operator reads the build by scrolling the form.
 */
function Step({ title, children }: { title: string; children: ReactNode }) {
  return (
    <li>
      <p className="t-caps">{title}</p>
      {children}
    </li>
  );
}

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

/**
 * Every dependency as one chip in one field: what installs it, what it is,
 * which version, and the options mise reads on it. The component owns the
 * box, the suggestions and the keyboard; what a mise key looks like and which
 * options a tool accepts stay here.
 */
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
  const typed = query.trim();
  const matches = suggest(tools, query, value.map((dep) => dep.tool));
  // The catalog is a thousand tools; marking the typed entry must not rebuild
  // that set on every keystroke.
  const known = useMemo(
    () => new Set(tools.flatMap((tool) => [tool.name, ...tool.backends])),
    [tools],
  );
  const unversioned = value.filter((dep) => !isVersion(dep.version));

  useEffect(() => {
    let active = true;
    setLoading(true);
    void loadToolCatalog()
      .then((results) => { if (active) setTools(results); })
      .catch(() => { if (active) setSearchFailed(true); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, []);

  // A new dependency starts at latest because that is what most recipes want,
  // and the version is one press away in the chip itself.
  function add(tool: string) {
    setQuery("");
    if (value.some((dep) => dep.tool === tool)) return;
    onChange([...value, { tool, version: "latest" }]);
  }

  function update(tool: string, change: Partial<Dependency>) {
    onChange(value.map((dep) => dep.tool === tool ? { ...dep, ...change } : dep));
  }

  // allow_builds is the only option mise reads here, so an entry naming
  // anything else is left alone rather than written into the recipe.
  function setOption(tool: string, text: string) {
    const option = readOption(text);
    if (!option || option.name !== ALLOW_BUILDS) return;
    update(tool, { allow_builds: option.values.length ? option.values : undefined });
  }

  return (
    <div className="field">
      <Label htmlFor="dependency-search">Dependencies</Label>
      <ChipInput>
        <ChipInputBox
          disabled={disabled}
          onFocus={() => setFocused(true)}
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false);
          }}
        >
          {value.map((dep) => {
            const { scope, name } = splitKey(dep.tool);
            return (
              <Chip key={dep.tool} invalid={!isVersion(dep.version)}>
                {scope ? <ChipScope>{scope}</ChipScope> : null}
                <ChipName>{name}</ChipName>
                <ChipValue
                  value={dep.version}
                  editable={!disabled}
                  editLabel={`Edit ${dep.tool} version, currently ${dep.version}`}
                  confirmLabel={`Confirm ${dep.tool} version`}
                  onCommit={(version) => update(dep.tool, { version })}
                />
                {dep.allow_builds?.length ? (
                  <ChipOption
                    name={ALLOW_BUILDS}
                    value={dep.allow_builds}
                    label={dep.tool}
                    editable={!disabled}
                    onCommit={(text) => setOption(dep.tool, text)}
                  />
                ) : takesAllowBuilds(dep.tool) && !disabled ? (
                  <ChipOptionAdd label={dep.tool} onCommit={(text) => setOption(dep.tool, text)} />
                ) : null}
                <ChipRemove
                  aria-label={`Remove ${dep.tool}`}
                  disabled={disabled}
                  onClick={() => onChange(value.filter((item) => item.tool !== dep.tool))}
                />
              </Chip>
            );
          })}
          <ChipInputField
            id="dependency-search"
            aria-describedby="dependency-help"
            placeholder="node, claude, npm:t3..."
            disabled={disabled}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onRemoveLast={() => onChange(value.slice(0, -1))}
            onKeyDown={(event) => {
              // ChipInput has already taken the key when a suggestion was
              // highlighted, and taking it twice would add the tool twice.
              if (event.defaultPrevented) return;
              if (event.key === "Enter") {
                // This field collects values; the recipe is saved by its own
                // button. Enter here never reaches the form.
                event.preventDefault();
                if (isKey(typed)) add(typed);
              }
              if (event.key === "Escape") { event.preventDefault(); setQuery(""); }
            }}
          />
        </ChipInputBox>
        {focused && typed ? (
          <ChipInputList aria-label="Tool suggestions">
            {matches.map((tool) => (
              <ChipInputOption key={tool} onSelect={() => add(tool)}>
                <span className="mono">{tool}</span>
                {known.has(tool) ? null : <span className="muted">as typed</span>}
              </ChipInputOption>
            ))}
            {matches.length ? null : (
              <ChipInputEmpty>
                {loading
                  ? "Searching mise..."
                  : searchFailed
                    ? "Catalog unavailable. Type a mise key, such as just or npm:t3."
                    : "No match. Try a backend key, such as npm:t3."}
              </ChipInputEmpty>
            )}
          </ChipInputList>
        ) : null}
      </ChipInput>
      <small className="field-hint" id="dependency-help">
        Type a mise key or search for one. Enter and Tab take a suggestion, Backspace removes the
        last chip. Press a version to change it. npm tools take{" "}
        <code>allow_builds=name, name</code> on their <code>+</code>.
      </small>
      {unversioned.length ? (
        <ValidationMessage>
          A version is letters, digits, dots and dashes:{" "}
          {unversioned.map((dep) => dep.tool).join(", ")}.
        </ValidationMessage>
      ) : null}
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
  const [setup, setSetup] = useState("");
  const [buildChecks, setBuildChecks] = useState("");
  // A string is the file the Operator took over; null leaves it to the Host.
  const [dockerfile, setDockerfile] = useState<string | null>(null);
  const [takingOver, setTakingOver] = useState(false);
  const [discarding, setDiscarding] = useState(false);
  const [dock, setDock] = useState<Dock>("closed");
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
  const manual = dockerfile !== null;
  const buildable = dependencies.length > 0 && dependencies.every((dep) => isVersion(dep.version));
  const ready = manual ? !!dockerfile.trim() : buildable;
  const lastLine = current?.log.trimEnd().split("\n").at(-1) ?? "No build yet.";

  useEffect(() => {
    setName(current?.name ?? "");
    setTemplateId(current?.template_id ?? "");
    setDependencies(current?.dependencies ?? []);
    setSetup((current?.setup ?? []).join("\n"));
    setBuildChecks((current?.build_checks ?? []).join("\n"));
    setDockerfile(current?.dockerfile ?? null);
    setDiscarding(false);
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
    setSetup((template?.setup ?? []).join("\n"));
    setBuildChecks((template?.buildChecks ?? []).join("\n"));
    setEditorKey((value) => value + 1);
  }

  // A build the Operator just started is the one thing worth interrupting the
  // form for. A dock they closed since stays closed.
  useEffect(() => {
    if (current?.status === "building") setDock((dock) => dock === "closed" ? "half" : dock);
  }, [current?.status]);

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

  function fields() {
    return recipeBody({ name, templateId, dependencies, setup, buildChecks, dockerfile });
  }

  /**
   * The Host renders the file, so the console never carries a copy of the
   * template. A failure leaves the builder exactly as it was.
   */
  async function takeOver() {
    setTakingOver(true);
    setFailure(null);
    try {
      const response = await api("/dev-images/dockerfile", {
        method: "POST",
        body: JSON.stringify(recipeBody({ name, templateId, dependencies, setup, buildChecks, dockerfile: null })),
      });
      if (!response.ok) throw await failureOf(response);
      setDockerfile(await response.text());
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setTakingOver(false);
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    setSubmitting(true);
    setFailure(null);
    try {
      const response = await api("/dev-images", {
        method: "POST",
        body: JSON.stringify({ id: selected, ...fields() }),
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
                      <TableCell className="mono">
                        {image.dockerfile ? <span className="muted">Custom Dockerfile</span>
                          : image.dependencies.map((dep) => `${dep.tool}@${dep.version}`).join(", ")}
                      </TableCell>
                      <TableCell className="mono">{image.image}</TableCell>
                      <TableCell><StatusBadge tone={statusTone(image)}>{statusLabel(image)}</StatusBadge></TableCell>
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
      <Card className="dev-images-panel" data-dock={dock}>
        <form className="dev-image-form" onSubmit={submit}>
          <div className="row">
            <p className="t-caps grow">{manual ? "Dockerfile" : "Configuration"}</p>
            {manual ? <Badge variant="warning">Edited by hand</Badge> : null}
            {current ? <StatusBadge tone={statusTone(current)}>{statusLabel(current)}</StatusBadge> : null}
          </div>
          {manual ? (
            <>
              <FormField id="dev-image-name" label="Image name" placeholder="web-dev"
                value={name} onChange={(event) => setName(event.target.value)}
                required maxLength={128} disabled={editingDisabled}
              />
              <FormField id="dev-image-dockerfile" label="Dockerfile"
                hint="The Host builds this file as is. Keep the dev user and the entrypoint, or the container will not start. Build checks only run if you keep their RUN --network=none step.">
                <ShellEditor id="dev-image-dockerfile" value={dockerfile} onChange={setDockerfile}
                  disabled={editingDisabled} maxLength={65536} />
              </FormField>
              <div className="dev-image-takeover">
                {discarding ? (
                  <>
                    <p className="grow t-label">Discard this file and go back to the builder? It is written again from the fields, which still hold what they held before.</p>
                    <Button type="button" size="sm" variant="ghost" onClick={() => setDiscarding(false)}>Keep editing</Button>
                    <Button type="button" size="sm" className="btn-danger"
                      onClick={() => { setDiscarding(false); setDockerfile(null); }}>Discard edits</Button>
                  </>
                ) : (
                  <>
                    <p className="grow muted t-label">
                      {dockerfile.split("\n").length} lines. Going back to the builder discards them.
                    </p>
                    <Button type="button" size="sm" variant="ghost" disabled={editingDisabled}
                      onClick={() => setDiscarding(true)}>Back to builder</Button>
                  </>
                )}
              </div>
            </>
          ) : (
          <>
          <ol className="dev-image-steps">
            <Step title="Image">
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
                hint="Debian 13 with mise and Git. The name becomes the local tag."
                value={name} onChange={(event) => setName(event.target.value)}
                required maxLength={128} disabled={editingDisabled}
              />
            </Step>
            <Step title="Dependencies">
              <Dependencies key={editorKey} value={dependencies} onChange={setDependencies} disabled={editingDisabled} />
            </Step>
            <Step title="Setup">
              <FormField id="dev-image-setup" label="Setup commands (optional)"
                hint="One command per line, each its own build step. Runs as root, with network, after the dependencies. This is where a shell installer or an apt package goes.">
                <ShellEditor id="dev-image-setup" value={setup} onChange={setSetup}
                  disabled={editingDisabled} maxLength={65536}
                  placeholder={"curl -fsSL https://example.com/install.sh | bash\napt-get install -y --no-install-recommends ripgrep"} />
              </FormField>
            </Step>
            <Step title="Build checks">
              <FormField id="build-checks" label="Build checks (optional)"
                hint="One command per line, saved as tasks.check.run. Runs as dev without network access, after the setup, with a 60-second limit per command. Any failure stops the build.">
                <ShellEditor id="build-checks" value={buildChecks} onChange={setBuildChecks}
                  disabled={editingDisabled} maxLength={65536}
                  placeholder={"t3 --help\nclaude --version\ncodex --version"} />
              </FormField>
            </Step>
          </ol>
          <div className="dev-image-takeover">
            <div className="grow">
              <p className="t-label">Need more than these fields?</p>
              <p className="muted t-label">Open the generated Dockerfile and edit it directly. The builder switches off for this image, and the Host builds exactly what you write.</p>
            </div>
            <Button type="button" size="sm" disabled={editingDisabled || takingOver || !buildable}
              onClick={() => void takeOver()}>
              {takingOver ? "Opening..." : "Edit Dockerfile"}
            </Button>
          </div>
          </>
          )}
          {failure ? <Failure failure={failure} /> : null}
          {current?.last_error ? <Failure failure={current.last_error} /> : null}
          {building ? <p className="muted t-label">A build is running on this Host.</p> : null}
          <div className="form-actions">
            <Button type="submit" variant="primary" disabled={submitting || building || !loaded || !!loadFailure || !ready || (!!selected && !current)}>
              {submitting ? "Saving..." : current?.status === "building" ? "Building..." : "Save and build"}
            </Button>
          </div>
        </form>
        <div className="dev-image-dock" role="region" aria-label="Build logs">
          <div className="dev-image-dock-head">
            <Button type="button" size="sm" variant="ghost" aria-expanded={dock !== "closed"}
              onClick={() => setDock(dock === "closed" ? "half" : "closed")}>Logs</Button>
            {current ? <StatusBadge tone={statusTone(current)}>{statusLabel(current)}</StatusBadge> : null}
            <span className="grow mono muted t-label dev-image-lastline">{lastLine}</span>
            {dock !== "closed" ? (
              <Button type="button" size="sm" variant="ghost"
                onClick={() => setDock(dock === "full" ? "half" : "full")}>
                {dock === "full" ? "Restore" : "Expand"}
              </Button>
            ) : null}
          </div>
          {dock !== "closed" ? (
            <LogView key={current?.id ?? "new"} role="log" aria-live="polite" tabIndex={0} aria-label="Build logs">
              {(current?.log || "Save and build to see the output here.").split("\n").map((line, index) => (
                <LogViewLine key={index}>{line || " "}</LogViewLine>
              ))}
            </LogView>
          ) : null}
        </div>
      </Card>
    </div>
  );
}
