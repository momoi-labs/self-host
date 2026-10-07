import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  StatusBadge,
  Button, Card,
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle, AlertDialogDescription,
  AlertDialogFooter, AlertDialogCancel, AlertDialogAction,
  EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateIcon, EmptyStateTitle,
  PageHeader, PageHeaderDescription, PageHeaderTitle,
  Search,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { CustomImageEditor } from "../components/CustomImageEditor.js";
import { Failure } from "../components/Failure.js";
import { Icon } from "../components/Icon.js";
import { api, asReport, failureOf } from "../lib/api.js";
import { customImagesQuery } from "../lib/queries.js";
import { buildLabel, buildTone } from "../lib/status.js";
import { waitForTask } from "../lib/tasks.js";
import type { CustomImage, Report } from "../lib/types.js";
import { fetchEvents } from "../lib/useEvents.js";

const noImages: CustomImage[] = [];

/**
 * The saved images and their latest build. This owns the records and the
 * poll that refreshes them; the editor owns one recipe's fields.
 */
export function CustomImages({ listing, selected, onOpen, onList }: {
  listing: boolean;
  selected: string | null;
  onOpen: (id: string | null) => void;
  onList: () => void;
}) {
  const queryClient = useQueryClient();
  const list = useQuery({ ...customImagesQuery, refetchInterval: 1000 });
  const images = list.data ?? noImages;
  const loaded = list.data !== undefined;
  const loadFailure = list.error ? asReport(list.error) : null;
  const [failure, setFailure] = useState<Report | null>(null);
  const [query, setQuery] = useState("");
  const [confirming, setConfirming] = useState<CustomImage | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const current = images.find((image) => image.id === selected);

  /** A saved recipe is the record the editor reopens, so it lands here before
   * the next poll does. */
  function saved(image: CustomImage) {
    queryClient.setQueryData(customImagesQuery.queryKey, (records = noImages) => [image, ...records.filter((item) => item.id !== image.id)]);
    onOpen(image.id);
    void list.refetch();
  }

  const visible = images.filter((image) => image.name.toLowerCase().includes(query.trim().toLowerCase()));

  async function remove(image: CustomImage) {
    setDeleting(image.id);
    setFailure(null);
    try {
      const response = await api(`/custom-images/${encodeURIComponent(image.id)}`, { method: "DELETE" });
      if (!response.ok) throw await failureOf(response);
      // Queued behind a build of the same image, if one is running.
      const { task_id } = (await response.json()) as { task_id: string };
      const outcome = await waitForTask(task_id, { events: fetchEvents });
      if (outcome.status === "failed") throw outcome.error ?? new Error("Could not delete the image.");
      queryClient.setQueryData(customImagesQuery.queryKey, (records = noImages) => records.filter((item) => item.id !== image.id));
      void list.refetch();
    } catch (cause) {
      setFailure(asReport(cause));
    } finally {
      setDeleting(null);
      setConfirming(null);
    }
  }

  function deleteReason(image: CustomImage) {
    if (image.in_use) return "In use";
    if (image.in_use !== false) return "Usage unavailable";
    return null;
  }

  const confirmDialog = (
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
  );

  if (listing) return (
    <>
      <PageHeader actions={
        <Button size="sm" variant="primary" onClick={() => onOpen(null)}><Icon name="plus" />New image</Button>
      }>
        <PageHeaderTitle>Custom images</PageHeaderTitle>
        <PageHeaderDescription>Saved images and their latest build on this Host.</PageHeaderDescription>
      </PageHeader>
      {loadFailure ? <Failure failure={loadFailure} /> : null}
      {failure ? <Failure failure={failure} /> : null}
      <div className="list-filters">
        <Search aria-label="Search by name" placeholder="Search by name..." value={query}
          onChange={(event) => setQuery(event.target.value)} />
        {query ? <Button size="sm" variant="ghost" onClick={() => setQuery("")}>Clear filters</Button> : null}
      </div>
      {!loaded && !loadFailure ? <p className="muted">Loading images...</p> : loaded && !images.length ? (
        <Card>
          <EmptyState variant="first-run" className="hatch">
            <EmptyStateIcon><Icon name="box" size="lg" /></EmptyStateIcon>
            <EmptyStateTitle>No custom images yet</EmptyStateTitle>
            <EmptyStateDescription>Choose your mise dependencies and build your first custom image.</EmptyStateDescription>
            <EmptyStateActions><Button size="sm" variant="primary" onClick={() => onOpen(null)}><Icon name="plus" />New image</Button></EmptyStateActions>
          </EmptyState>
        </Card>
      ) : loaded ? (
        <div className="stack custom-image-list">
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
                      <TableCell><StatusBadge tone={buildTone(image)} pulse={image.status === "building"}>{buildLabel(image)}</StatusBadge></TableCell>
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
      {confirmDialog}
    </>
  );

  return (
    <>
      <CustomImageEditor current={current} selected={selected}
        loaded={loaded} loadFailure={loadFailure} onSaved={saved} onCancel={onList}
        onDelete={current ? () => setConfirming(current) : undefined}
        deleteReason={current ? deleteReason(current) : null}
        deleting={!!deleting} />
      {failure ? <Failure failure={failure} /> : null}
      {confirmDialog}
    </>
  );
}
