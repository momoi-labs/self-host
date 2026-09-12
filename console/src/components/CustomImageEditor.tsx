import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import {
  Badge, Button, Card, FormField, Label,
  Chip, ChipInput, ChipInputBox, ChipInputEmpty, ChipInputField, ChipInputList, ChipInputOption,
  ChipName, ChipOption, ChipOptionAdd, ChipRemove, ChipScope, ChipValue,
  PageHeader, PageHeaderDescription, PageHeaderTitle,
  LogView, LogViewLine,
  ValidationMessage,
} from "@momoi-labs/kiso-react";

import { Failure } from "./Failure.js";
import { ShellEditor } from "./ShellEditor.js";
import { StatusBadge } from "./StatusBadge.js";
import { api, asReport, failureOf } from "../lib/api.js";
import type { CustomImage, Report } from "../lib/types.js";
import { buildLabel, buildTone } from "../lib/status.js";
import { customImageTemplate, customImageTemplates, type ImageDependency } from "../lib/customImageTemplates.js";
import { recipeBody } from "../lib/customImageRecipe.js";
import {
  ALLOW_BUILDS, isKey, isVersion, readOption, splitKey, suggest, takesAllowBuilds,
} from "../lib/dependencies.js";

type Dependency = ImageDependency;
type MiseTool = { name: string; description?: string; backends: string[] };

/** How much of the card the build log takes: none, the lower half, or all. */
type Dock = "closed" | "half" | "full";

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
    const response = await api("/custom-images/tools");
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
        The packages mise installs into the image, one chip per tool with the version to install.
        They end up on the <code>PATH</code> inside the container, so they are what it can run.
        Anything mise has no package for goes in Custom commands below.
        <br />
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

/**
 * The recipe for one image, in either mode, with its build log docked below.
 * The listing owns the records and the polling; this owns the fields, which
 * is why a poll can refresh the log without touching an edit in progress.
 */
export function CustomImageEditor({ current, selected, building, loaded, loadFailure, onSaved }: {
  current: CustomImage | undefined;
  selected: string | null;
  /** Any image on the Host is building, so this one cannot start. */
  building: boolean;
  loaded: boolean;
  loadFailure: Report | null;
  onSaved: (image: CustomImage) => void;
}) {
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
  const [editorKey, setEditorKey] = useState(0);
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
  }, [selected, current?.id]);

  function chooseTemplate(id: string) {
    const template = customImageTemplate(id);
    const previous = customImageTemplate(templateId);
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

  /**
   * The Host renders the file, so the console never carries a copy of the
   * template. A failure leaves the builder exactly as it was.
   */
  async function takeOver() {
    setTakingOver(true);
    setFailure(null);
    try {
      const response = await api("/custom-images/dockerfile", {
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
      const response = await api("/custom-images", {
        method: "POST",
        body: JSON.stringify({
          id: selected,
          ...recipeBody({ name, templateId, dependencies, setup, buildChecks, dockerfile }),
        }),
      });
      if (!response.ok) throw await failureOf(response);
      const image = await response.json() as CustomImage;
      setName(image.name);
      onSaved(image);
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="custom-images-page">
      <PageHeader>
        <PageHeaderTitle>{selected ? current?.name ?? "Image" : "New image"}</PageHeaderTitle>
        <PageHeaderDescription>Save your image's dependencies and build it on this Host.</PageHeaderDescription>
      </PageHeader>
      {loadFailure ? <Failure failure={loadFailure} /> : null}
      <Card className="custom-images-panel" data-dock={dock}>
        <form className="custom-image-form" onSubmit={submit}>
          <div className="row">
            <p className="t-caps grow">{manual ? "Dockerfile" : "Configuration"}</p>
            {manual ? <Badge variant="warning">Edited by hand</Badge> : null}
            {current ? <StatusBadge tone={buildTone(current)}>{buildLabel(current)}</StatusBadge> : null}
          </div>
          {manual ? (
            <>
              <FormField id="custom-image-name" label="Image name" placeholder="web-dev"
                value={name} onChange={(event) => setName(event.target.value)}
                required maxLength={128} disabled={editingDisabled}
              />
              <FormField id="custom-image-dockerfile" label="Dockerfile"
                hint="The Host builds this file as is. Keep the dev user and the entrypoint, or the container will not start. Build checks only run if you keep their RUN --network=none step.">
                <ShellEditor id="custom-image-dockerfile" value={dockerfile} onChange={setDockerfile}
                  disabled={editingDisabled} maxLength={65536} />
              </FormField>
              <div className="custom-image-takeover">
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
              <ol className="custom-image-steps">
                <Step title="Image">
                  {!selected ? (
                    <FormField id="custom-image-template" label="Template"
                      hint="Start with a template, then edit any field before building.">
                      <select id="custom-image-template" className="input" value={templateId}
                        disabled={editingDisabled} onChange={(event) => chooseTemplate(event.target.value)}>
                        <option value="">No template</option>
                        {customImageTemplates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}
                      </select>
                    </FormField>
                  ) : null}
                  <FormField id="custom-image-name" label="Image name" placeholder="web-dev"
                    value={name} onChange={(event) => setName(event.target.value)}
                    required maxLength={128} disabled={editingDisabled}
                  />
                </Step>
                <Step title="mise packages and dependencies">
                  <Dependencies key={editorKey} value={dependencies} onChange={setDependencies} disabled={editingDisabled} />
                </Step>
                <Step title="Custom commands">
                  <FormField id="custom-image-setup" label="Custom commands"
                    hint="One command per line, each its own build step. Runs as root, with network, after the dependencies. This is where a shell installer or an apt package goes.">
                    <ShellEditor id="custom-image-setup" value={setup} onChange={setSetup}
                      disabled={editingDisabled} maxLength={65536}
                      placeholder={"curl -fsSL https://example.com/install.sh | bash\napt-get install -y --no-install-recommends ripgrep"} />
                  </FormField>
                </Step>
                <Step title="Build checks">
                  <FormField id="custom-image-checks" label="Build checks"
                    hint="One command per line, saved as tasks.check.run. Runs as dev without network access, after the custom commands, with a 60-second limit per command. Any failure stops the build.">
                    <ShellEditor id="custom-image-checks" value={buildChecks} onChange={setBuildChecks}
                      disabled={editingDisabled} maxLength={65536}
                      placeholder={"t3 --help\nclaude --version\ncodex --version"} />
                  </FormField>
                </Step>
              </ol>
              <div className="custom-image-takeover">
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
        <div className="custom-image-dock" role="region" aria-label="Build logs">
          <div className="custom-image-dock-head">
            <Button type="button" size="sm" variant="ghost" aria-expanded={dock !== "closed"}
              onClick={() => setDock(dock === "closed" ? "half" : "closed")}>Logs</Button>
            {current ? <StatusBadge tone={buildTone(current)}>{buildLabel(current)}</StatusBadge> : null}
            <span className="grow mono muted t-label custom-image-lastline">{lastLine}</span>
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
