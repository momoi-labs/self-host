// PROTOTYPE for momoi-labs/self-host#107. Five variants of the development
// image editor on the existing #new-dev-image route, switchable with
// ?variant=A..E. Every variant answers the same two questions: where does a
// Dockerfile builder live next to a "paste the file" escape hatch, and where
// do the build logs go once the form is too tall for a side-by-side split.
// Throwaway: `npm run prototype` in console/.

import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Badge, Button, Card, Dot, FormField, Label,
  Chip, ChipInput, ChipInputBox, ChipInputField, ChipName, ChipRemove, ChipValue,
  LogView, LogViewLine,
  PageHeader, PageHeaderDescription, PageHeaderTitle,
  Switch, Tabs, TabsContent, TabsList, TabsTrigger,
} from "@momoi-labs/kiso-react";

import { PrototypeSwitcher, readVariant } from "../components/PrototypeSwitcher.js";
import { ShellEditor } from "../components/ShellEditor.js";
import {
  dockerfileBlocks, sectionHints, sectionTitles, templates, usePrototype,
  type Prototype, type Section, type Status,
} from "../lib/devImagePrototype.js";
import "../prototype.css";

const variants = [
  { key: "A", name: "Sections, edit Dockerfile, log dock" },
  { key: "B", name: "Three tabs" },
  { key: "C", name: "Live preview, logs below" },
  { key: "D", name: "Wizard" },
  { key: "E", name: "Annotated Dockerfile" },
];

export function DevImagePrototype() {
  const [variant, setVariant] = useState(() => readVariant() ?? "A");
  const p = usePrototype();
  const Variant = { A: VariantA, B: VariantB, C: VariantC, D: VariantD, E: VariantE }[variant] ?? VariantA;
  return (
    <div className="dev-images-page proto-page">
      <PageHeader>
        <PageHeaderTitle>New image</PageHeaderTitle>
        <PageHeaderDescription>Describe the image and build it on this Host.</PageHeaderDescription>
      </PageHeader>
      <Variant p={p} />
      <PrototypeSwitcher variants={variants} current={variant} onChange={setVariant}
        state={{ source: p.source, status: p.status, recipe: p.recipe, dockerfile: p.source === "dockerfile" ? p.dockerfile : "(generated)" }} />
    </div>
  );
}

/* ---------- shared pieces ---------- */

const statusTone: Record<Status, "neutral" | "success" | "danger" | "info"> = { idle: "neutral", building: "info", ready: "success", failed: "danger" };
const statusLabel: Record<Status, string> = { idle: "Not built", building: "Building", ready: "Built", failed: "Failed" };

function StatusPill({ status }: { status: Status }) {
  return <Badge variant={statusTone[status]}><Dot />{statusLabel[status]}</Badge>;
}

function SourceToggle({ p, compact }: { p: Prototype; compact?: boolean }) {
  return (
    <div className="segmented" role="tablist" aria-label="Recipe source">
      <button type="button" role="tab" aria-selected={p.source === "builder"} onClick={() => p.changeSource("builder")}>Builder</button>
      <button type="button" role="tab" aria-selected={p.source === "dockerfile"} onClick={() => p.changeSource("dockerfile")}>
        {compact ? "Dockerfile" : "Paste a Dockerfile"}
      </button>
    </div>
  );
}

function Deps({ p, disabled }: { p: Prototype; disabled?: boolean }) {
  const [query, setQuery] = useState("");
  const value = p.recipe.dependencies;
  function add(text: string) {
    const [tool, version = "latest"] = text.split("@");
    if (!tool || value.some((dep) => dep.tool === tool)) return;
    p.setRecipe({ dependencies: [...value, { tool, version }] });
    setQuery("");
  }
  return (
    <div className="field">
      <Label htmlFor="proto-deps">Dependencies</Label>
      <ChipInput>
        <ChipInputBox disabled={disabled}>
          {value.map((dep) => (
            <Chip key={dep.tool}>
              <ChipName>{dep.tool}</ChipName>
              <ChipValue value={dep.version} editable={!disabled} editLabel={`Edit ${dep.tool} version`} confirmLabel="Confirm"
                onCommit={(version) => p.setRecipe({ dependencies: value.map((item) => item.tool === dep.tool ? { ...item, version } : item) })} />
              <ChipRemove aria-label={`Remove ${dep.tool}`} disabled={disabled}
                onClick={() => p.setRecipe({ dependencies: value.filter((item) => item.tool !== dep.tool) })} />
            </Chip>
          ))}
          <ChipInputField id="proto-deps" placeholder="node@24, uv, npm:t3..." disabled={disabled} value={query}
            onChange={(event) => setQuery(event.target.value)}
            onRemoveLast={() => p.setRecipe({ dependencies: value.slice(0, -1) })}
            onKeyDown={(event) => {
              if (event.key === "Enter") { event.preventDefault(); add(query.trim()); }
            }} />
        </ChipInputBox>
      </ChipInput>
      <small className="field-hint">{sectionHints.dependencies} Enter adds a chip; press a version to change it.</small>
    </div>
  );
}

/** One section of the builder. Variants decide where each one sits. */
function SectionField({ p, section, disabled, onFocus }: { p: Prototype; section: Section; disabled?: boolean; onFocus?: () => void }) {
  const stop = disabled || p.status === "building";
  return (
    <div className="proto-section" data-section={section} onFocusCapture={onFocus}>
      {section === "image" ? (
        <>
          <FormField id="proto-template" label="Template" hint="Start from a template, then edit any field.">
            <select id="proto-template" className="input" value={p.recipe.templateId} disabled={stop}
              onChange={(event) => p.chooseTemplate(event.target.value)}>
              <option value="">Custom image</option>
              {templates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}
            </select>
          </FormField>
          <FormField id="proto-name" label="Image name" placeholder="web-dev" value={p.recipe.name} disabled={stop}
            onChange={(event) => p.setRecipe({ name: event.target.value })} hint={sectionHints.image} />
        </>
      ) : section === "dependencies" ? (
        <Deps p={p} disabled={stop} />
      ) : section === "setup" ? (
        <FormField id="proto-setup" label="Setup (optional)" hint={sectionHints.setup}>
          <ShellEditor id="proto-setup" value={p.recipe.setup} disabled={stop}
            onChange={(setup) => p.setRecipe({ setup })}
            placeholder={"curl -fsSL https://example.com/install.sh | bash\nuv tool install ruff"} />
        </FormField>
      ) : (
        <FormField id="proto-checks" label="Build checks (optional)" hint={sectionHints.checks}>
          <ShellEditor id="proto-checks" value={p.recipe.buildChecks} disabled={stop}
            onChange={(buildChecks) => p.setRecipe({ buildChecks })}
            placeholder={"hermes --version\nclaude --version"} />
        </FormField>
      )}
    </div>
  );
}

function DockerfileField({ p, rows = 24 }: { p: Prototype; rows?: number }) {
  return (
    <div className="proto-dockerfile-field" style={{ "--rows": rows } as React.CSSProperties}>
      <FormField id="proto-dockerfile" label="Dockerfile"
        hint="Your file replaces the generated one. The Host still adds its entrypoint and the dev user; keep FROM debian:13-slim or the runtime will not start.">
        <ShellEditor id="proto-dockerfile" value={p.dockerfile} onChange={p.setDockerfile}
          disabled={p.status === "building"} placeholder="FROM debian:13-slim" />
      </FormField>
      <div className="row">
        <Button type="button" size="sm" variant="ghost" onClick={() => p.setDockerfile(p.generated)}>Reset to generated</Button>
        <span className="muted t-label">{p.dockerfile.split("\n").length} lines</span>
      </div>
    </div>
  );
}

function BuildButton({ p, label }: { p: Prototype; label?: string }) {
  return (
    <Button type="button" variant="primary" disabled={!p.canBuild} onClick={p.build}>
      {p.status === "building" ? "Building..." : label ?? "Save and build"}
    </Button>
  );
}

function Log({ p, className }: { p: Prototype; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const scroller = ref.current?.querySelector(".log-scroll");
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [p.log.length]);
  return (
    <div ref={ref} className={`proto-log ${className ?? ""}`}>
      <LogView role="log" aria-live="polite" tabIndex={0} aria-label="Build logs">
        {(p.log.length ? p.log : ["Save and build to see the output here."]).map((line, index) => (
          <LogViewLine key={index} className={line.includes("ERROR") ? "log-error" : undefined}>{line || " "}</LogViewLine>
        ))}
      </LogView>
    </div>
  );
}

function LastLine({ p }: { p: Prototype }) {
  const line = p.log.at(-1) ?? "";
  return <span className="mono muted proto-lastline">{p.status === "idle" ? "No build yet" : line}</span>;
}

/* ---------- A: sectioned form with a log dock ---------- */

function VariantA({ p }: { p: Prototype }) {
  const [dock, setDock] = useState<"closed" | "half" | "full">("closed");
  const [discarding, setDiscarding] = useState(false);
  useEffect(() => { if (p.status === "building" && dock === "closed") setDock("half"); }, [p.status]); // eslint-disable-line react-hooks/exhaustive-deps
  const manual = p.source === "dockerfile";
  return (
    <Card className="proto-a" data-dock={dock}>
      <div className="proto-a-form">
        <div className="row proto-toolbar">
          <p className="t-caps grow">{manual ? "Dockerfile" : "Configuration"}</p>
          {manual ? <Badge variant="warning">Edited by hand</Badge> : null}
          <StatusPill status={p.status} />
        </div>
        {!manual ? (
          <>
            <ol className="proto-steps">
              {(["image", "dependencies", "setup", "checks"] as Section[]).map((section, index) => (
                <li key={section}>
                  <div className="proto-step-head"><span className="proto-step-num">{index + 1}</span><span className="t-caps">{sectionTitles[section]}</span></div>
                  <SectionField p={p} section={section} />
                </li>
              ))}
            </ol>
            <div className="proto-a-takeover">
              <div className="grow">
                <p className="t-label">Need more than these fields?</p>
                <p className="muted t-label">Open the generated Dockerfile and edit it directly. The builder switches off for this image; the Host builds exactly what you write.</p>
              </div>
              <Button type="button" size="sm" variant="default" disabled={p.status === "building"} onClick={() => p.changeSource("dockerfile")}>Edit Dockerfile</Button>
            </div>
          </>
        ) : (
          <>
            <FormField id="proto-name" label="Image name" placeholder="web-dev" value={p.recipe.name}
              disabled={p.status === "building"} onChange={(event) => p.setRecipe({ name: event.target.value })} />
            <div className="proto-dockerfile-field" style={{ "--rows": 28 } as React.CSSProperties}>
              <FormField id="proto-dockerfile" label="Dockerfile"
                hint="The Host builds this file as is. Keep the dev user and the entrypoint, or the container will not start. Build checks only run if you keep their RUN --network=none step.">
                <ShellEditor id="proto-dockerfile" value={p.dockerfile} onChange={p.setDockerfile}
                  disabled={p.status === "building"} placeholder="FROM debian:13-slim" />
              </FormField>
            </div>
            <div className="proto-a-takeover">
              <div className="grow">
                {discarding
                  ? <p className="t-label">Discard your edits and go back to the builder? The file will be regenerated from the fields.</p>
                  : <p className="muted t-label">{p.dockerfile.split("\n").length} lines. Going back to the builder discards these edits.</p>}
              </div>
              {discarding ? (
                <>
                  <Button type="button" size="sm" variant="ghost" onClick={() => setDiscarding(false)}>Keep editing</Button>
                  <Button type="button" size="sm" className="btn-danger" onClick={() => { setDiscarding(false); p.setDockerfile(""); p.changeSource("builder"); }}>Discard edits</Button>
                </>
              ) : (
                <Button type="button" size="sm" variant="ghost" disabled={p.status === "building"} onClick={() => setDiscarding(true)}>Back to builder</Button>
              )}
            </div>
          </>
        )}
        <div className="form-actions">
          <BuildButton p={p} />
        </div>
      </div>
      <div className="proto-a-dock" role="region" aria-label="Build logs">
        <div className="proto-a-dock-head">
          <button type="button" className="proto-plain" onClick={() => setDock(dock === "closed" ? "half" : "closed")}
            aria-expanded={dock !== "closed"}>{dock === "closed" ? "▲" : "▼"} Logs</button>
          <StatusPill status={p.status} />
          <span className="grow proto-clip"><LastLine p={p} /></span>
          <button type="button" className="proto-plain" onClick={() => setDock(dock === "full" ? "half" : "full")}
            aria-label={dock === "full" ? "Restore" : "Maximize"}>{dock === "full" ? "⤓" : "⤒"}</button>
        </div>
        {dock !== "closed" ? <Log p={p} /> : null}
      </div>
    </Card>
  );
}

/* ---------- B: three tabs ---------- */

function VariantB({ p }: { p: Prototype }) {
  const [tab, setTab] = useState("builder");
  const [detached, setDetached] = useState(false);
  useEffect(() => { if (p.status === "building") setTab("logs"); }, [p.status]);
  function toggleDetached(next: boolean) {
    setDetached(next);
    p.changeSource(next ? "dockerfile" : "builder");
  }
  return (
    <Card className="proto-b">
      <Tabs value={tab} onValueChange={setTab} className="proto-b-tabs">
        <TabsList className="tabs">
          <TabsTrigger value="builder" disabled={detached}>Builder{detached ? " (off)" : ""}</TabsTrigger>
          <TabsTrigger value="dockerfile">Dockerfile{detached ? <Badge variant="warning" className="proto-tab-badge">edited</Badge> : null}</TabsTrigger>
          <TabsTrigger value="logs">Logs {p.status !== "idle" ? <Badge variant={statusTone[p.status]} className="proto-tab-badge"><Dot />{p.log.length}</Badge> : null}</TabsTrigger>
        </TabsList>
        <TabsContent value="builder" className="proto-b-panel">
          <div className="stack">
            {(["image", "dependencies", "setup", "checks"] as Section[]).map((section) => (
              <section key={section} className="proto-fieldset">
                <p className="t-caps">{sectionTitles[section]}</p>
                <SectionField p={p} section={section} />
              </section>
            ))}
          </div>
        </TabsContent>
        <TabsContent value="dockerfile" className="proto-b-panel">
          <div className="stack">
            <div className="row">
              <Label htmlFor="proto-detach" className="grow">Edit the Dockerfile by hand. The builder tab switches off until you turn this back.</Label>
              <Switch id="proto-detach" checked={detached} onCheckedChange={toggleDetached} />
            </div>
            {detached ? <DockerfileField p={p} rows={30} /> : <pre className="dev-image-code proto-pre">{p.generated}</pre>}
          </div>
        </TabsContent>
        <TabsContent value="logs" className="proto-b-panel proto-b-logs">
          <Log p={p} />
        </TabsContent>
      </Tabs>
      <div className="proto-b-footer">
        <StatusPill status={p.status} />
        <span className="grow proto-clip"><LastLine p={p} /></span>
        <BuildButton p={p} />
      </div>
    </Card>
  );
}

/* ---------- C: form + live Dockerfile preview, logs below ---------- */

function VariantC({ p }: { p: Prototype }) {
  const [active, setActive] = useState<Section | null>(null);
  const [tall, setTall] = useState(false);
  useEffect(() => { if (p.status === "building") setTall(true); }, [p.status]);
  const blocks = dockerfileBlocks(p.recipe);
  return (
    <div className="proto-c" data-tall={tall}>
      <Card className="proto-c-form">
        <div className="row proto-toolbar">
          <p className="t-caps grow">Recipe</p>
          <SourceToggle p={p} compact />
        </div>
        {p.source === "builder" ? (
          <div className="stack">
            {(["image", "dependencies", "setup", "checks"] as Section[]).map((section) => (
              <SectionField key={section} p={p} section={section} onFocus={() => setActive(section)} />
            ))}
          </div>
        ) : <p className="muted">The builder is off. The Dockerfile on the right is what builds.</p>}
        <div className="form-actions"><BuildButton p={p} /></div>
      </Card>
      <Card className="proto-c-preview">
        <div className="row proto-toolbar">
          <p className="t-caps grow">Dockerfile {p.source === "builder" ? <span className="muted">(generated, read only)</span> : <span className="muted">(yours)</span>}</p>
          {p.source === "builder"
            ? <Button type="button" size="sm" variant="ghost" onClick={() => p.changeSource("dockerfile")}>Edit by hand</Button>
            : <Button type="button" size="sm" variant="ghost" onClick={() => p.changeSource("builder")}>Back to builder</Button>}
        </div>
        {p.source === "builder" ? (
          <pre className="proto-pre proto-c-blocks">
            {blocks.map((block, index) => (
              <span key={index} className="proto-block" data-active={block.section === active} data-section={block.section}>{block.text}{"\n\n"}</span>
            ))}
          </pre>
        ) : <DockerfileField p={p} rows={28} />}
      </Card>
      <Card className="proto-c-logs">
        <div className="row proto-toolbar">
          <p className="t-caps">Build log</p>
          <StatusPill status={p.status} />
          <span className="grow" />
          <Button type="button" size="sm" variant="ghost" onClick={() => setTall((value) => !value)}>{tall ? "Shorter" : "Taller"}</Button>
        </div>
        <Log p={p} />
      </Card>
    </div>
  );
}

/* ---------- D: wizard ---------- */

type Step = { key: string; title: string; render: (p: Prototype) => ReactNode };

function VariantD({ p }: { p: Prototype }) {
  const [index, setIndex] = useState(0);
  const [built, setBuilt] = useState(false);
  const steps: Step[] = p.source === "builder"
    ? [
      { key: "source", title: "Source", render: () => <SourceStep p={p} /> },
      { key: "image", title: "Image", render: () => <SectionField p={p} section="image" /> },
      { key: "dependencies", title: "Dependencies", render: () => <SectionField p={p} section="dependencies" /> },
      { key: "setup", title: "Setup", render: () => <SectionField p={p} section="setup" /> },
      { key: "checks", title: "Build checks", render: () => <SectionField p={p} section="checks" /> },
      { key: "review", title: "Review", render: () => <pre className="proto-pre dev-image-code">{p.generated}</pre> },
    ]
    : [
      { key: "source", title: "Source", render: () => <SourceStep p={p} /> },
      { key: "image", title: "Image", render: () => <FormField id="proto-name" label="Image name" value={p.recipe.name} onChange={(event) => p.setRecipe({ name: event.target.value })} /> },
      { key: "dockerfile", title: "Dockerfile", render: () => <DockerfileField p={p} rows={22} /> },
    ];
  const step = steps[Math.min(index, steps.length - 1)]!;
  const last = index >= steps.length - 1;
  if (built) return (
    <Card className="proto-d proto-d-build">
      <div className="row proto-toolbar">
        <p className="t-caps grow">Build of {p.recipe.name || "unnamed"}</p>
        <StatusPill status={p.status} />
        <Button type="button" size="sm" variant="ghost" disabled={p.status === "building"} onClick={() => { setBuilt(false); setIndex(0); }}>Edit recipe</Button>
        <Button type="button" size="sm" variant="primary" disabled={!p.canBuild} onClick={p.build}>Build again</Button>
      </div>
      <Log p={p} />
    </Card>
  );
  return (
    <Card className="proto-d">
      <ol className="proto-d-rail" aria-label="Steps">
        {steps.map((entry, position) => (
          <li key={entry.key} data-state={position === index ? "current" : position < index ? "done" : "todo"}>
            <button type="button" className="proto-plain" onClick={() => setIndex(position)}>
              <span className="proto-step-num">{position + 1}</span>{entry.title}
            </button>
          </li>
        ))}
      </ol>
      <div className="proto-d-body">
        <p className="t-caps">{index + 1}. {step.title}</p>
        <div className="stack proto-d-step">{step.render(p)}</div>
        <div className="form-actions">
          <Button type="button" variant="ghost" disabled={index === 0} onClick={() => setIndex(index - 1)}>Back</Button>
          {last
            ? <Button type="button" variant="primary" disabled={!p.canBuild} onClick={() => { setBuilt(true); p.build(); }}>Save and build</Button>
            : <Button type="button" variant="primary" onClick={() => setIndex(index + 1)}>Next</Button>}
        </div>
      </div>
    </Card>
  );
}

function SourceStep({ p }: { p: Prototype }) {
  return (
    <div className="proto-d-choices">
      {([
        ["builder", "Builder", "Pick a template, dependencies, setup commands and checks. The Host writes the Dockerfile."],
        ["dockerfile", "Paste a Dockerfile", "Bring your own file. Nothing is generated; the Host only adds its runtime entrypoint."],
      ] as const).map(([key, title, text]) => (
        <button key={key} type="button" className="proto-choice" aria-pressed={p.source === key} onClick={() => p.changeSource(key)}>
          <strong>{title}</strong><span className="muted">{text}</span>
        </button>
      ))}
    </div>
  );
}

/* ---------- E: annotated Dockerfile with a log sheet ---------- */

function VariantE({ p }: { p: Prototype }) {
  const [sheet, setSheet] = useState(false);
  useEffect(() => { if (p.status === "building") setSheet(true); }, [p.status]);
  const blocks = dockerfileBlocks(p.recipe);
  return (
    <Card className="proto-e" data-sheet={sheet}>
      <div className="row proto-toolbar proto-e-head">
        <p className="t-caps grow">Dockerfile</p>
        <FormField id="proto-name-e" label="" placeholder="image name" value={p.recipe.name}
          onChange={(event) => p.setRecipe({ name: event.target.value })} className="proto-e-name" />
        <Label htmlFor="proto-takeover" className="muted t-label">Take over the file</Label>
        <Switch id="proto-takeover" checked={p.source === "dockerfile"} onCheckedChange={(on) => p.changeSource(on ? "dockerfile" : "builder")} />
        <BuildButton p={p} label="Build" />
      </div>
      <div className="proto-e-doc">
        {p.source === "dockerfile" ? <DockerfileField p={p} rows={30} /> : blocks.map((block, index) => (
          <div key={index} className="proto-e-block" data-section={block.section}>
            <pre className="proto-pre proto-e-code">{block.text}</pre>
            {block.section === "image" || block.section === "dependencies" || block.section === "setup" || block.section === "checks" ? (
              <div className="proto-e-slot">
                <p className="t-caps">{sectionTitles[block.section]}</p>
                {block.section === "image"
                  ? <FormField id="proto-template" label="Template">
                    <select id="proto-template" className="input" value={p.recipe.templateId} onChange={(event) => p.chooseTemplate(event.target.value)}>
                      <option value="">Custom image</option>
                      {templates.map((template) => <option key={template.id} value={template.id}>{template.name}</option>)}
                    </select>
                  </FormField>
                  : <SectionField p={p} section={block.section} />}
              </div>
            ) : null}
          </div>
        ))}
      </div>
      <div className="proto-e-sheet" role="region" aria-label="Build logs">
        <button type="button" className="proto-e-pill" onClick={() => setSheet((value) => !value)} aria-expanded={sheet}>
          <StatusPill status={p.status} />
          <span className="grow proto-clip"><LastLine p={p} /></span>
          <span>{sheet ? "▼" : "▲"}</span>
        </button>
        {sheet ? <Log p={p} /> : null}
      </div>
    </Card>
  );
}
