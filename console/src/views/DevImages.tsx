import { useEffect, useState, type FormEvent } from "react";
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
import { Dependencies } from "../components/Dependencies.js";
import { Icon } from "../components/Icon.js";
import { ShellEditor } from "../components/ShellEditor.js";
import { StatusBadge } from "../components/StatusBadge.js";
import { api, asReport, failureOf } from "../lib/api.js";
import type { Report } from "../lib/types.js";
import { devImageTemplate, devImageTemplates, type ImageDependency } from "../lib/devImageTemplates.js";
import { isVersion, miseToml } from "../lib/dependencies.js";

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
  const mise = miseToml(dependencies, checks);
  const ready = dependencies.length > 0 && dependencies.every((dep) => isVersion(dep.version));

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
                <Button type="submit" variant="primary" disabled={submitting || building || !loaded || !!loadFailure || !ready || (!!selected && !current)}>
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
