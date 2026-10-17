import { useSyncExternalStore } from "react";

import { apiKey } from "./api.js";
import type { ArchiveFormat } from "./archiveFormat.js";

/**
 * Database import uploads, kept outside any screen so closing the import
 * dialog or moving to another console page does not stop one. The browser
 * sends the bytes, so a refresh or a closed tab does; the page warns first.
 */
export type ImportUpload = {
  key: string;
  databaseId: string;
  connectionId: string;
  file: File;
  format: ArchiveFormat;
  /** One transaction that stops at the first error. */
  allOrNothing: boolean;
  loaded: number;
  status: "uploading" | "failed";
  error: string | null;
};

/** What a finished upload hands back: the import's Task. */
export type FinishedUpload = { databaseId: string; taskId: string };

let uploads: ImportUpload[] = [];
const listeners = new Set<() => void>();
const finishedListeners = new Set<(finished: FinishedUpload) => void>();

function publish(next: ImportUpload[]) {
  uploads = next;
  for (const listener of listeners) listener();
}

function update(key: string, change: Partial<ImportUpload>) {
  publish(uploads.map((upload) => (upload.key === key ? { ...upload, ...change } : upload)));
}

function send(upload: ImportUpload) {
  const request = new XMLHttpRequest();
  request.open("POST", `/databases/${upload.databaseId}/connections/${upload.connectionId}/import`);
  request.setRequestHeader("Authorization", `Bearer ${apiKey()}`);
  request.setRequestHeader("Content-Type", "application/octet-stream");
  request.setRequestHeader("X-Upload-Name", encodeURIComponent(upload.file.name));
  request.setRequestHeader("X-Import-Format", upload.format);
  request.setRequestHeader("X-Import-All-Or-Nothing", String(upload.allOrNothing));
  request.upload.onprogress = (event) => update(upload.key, { loaded: event.loaded });
  request.onload = () => {
    if (request.status >= 200 && request.status < 300) {
      const accepted = JSON.parse(request.responseText) as { task_id: string };
      publish(uploads.filter((item) => item.key !== upload.key));
      for (const listener of finishedListeners) listener({ databaseId: upload.databaseId, taskId: accepted.task_id });
      return;
    }
    let error = `The server refused the upload (${request.status})`;
    try {
      error = (JSON.parse(request.responseText) as { error?: string }).error ?? error;
    } catch { /* not JSON */ }
    update(upload.key, { status: "failed", error });
  };
  request.onerror = () => update(upload.key, { status: "failed", error: "The connection dropped during the upload" });
  request.send(upload.file);
}

/** Starts sending `file` and answers its key; progress shows in `useImportUploads`. */
export function startUpload(databaseId: string, connectionId: string, file: File, format: ArchiveFormat, allOrNothing: boolean): string {
  const upload: ImportUpload = {
    key: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    databaseId, connectionId, file, format, allOrNothing, loaded: 0, status: "uploading", error: null,
  };
  publish([...uploads, upload]);
  send(upload);
  return upload.key;
}

/** Sends a failed upload again, from the start: the file is still in this page. */
export function retryUpload(key: string) {
  const upload = uploads.find((item) => item.key === key);
  if (!upload || upload.status !== "failed") return;
  const again = { ...upload, loaded: 0, status: "uploading" as const, error: null };
  publish(uploads.map((item) => (item.key === key ? again : item)));
  send(again);
}

export function dismissUpload(key: string) {
  publish(uploads.filter((item) => item.key !== key || item.status === "uploading"));
}

/** Calls `listener` whenever an upload becomes an import. */
export function onUploadFinished(listener: (finished: FinishedUpload) => void): () => void {
  finishedListeners.add(listener);
  return () => finishedListeners.delete(listener);
}

/** This database's uploads that have not become an import yet. */
export function useImportUploads(databaseId: string): ImportUpload[] {
  const all = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => uploads,
  );
  return all.filter((upload) => upload.databaseId === databaseId);
}

// Leaving the page would stop an upload the browser is still sending.
if (typeof window !== "undefined") {
  window.addEventListener("beforeunload", (event) => {
    if (uploads.some((upload) => upload.status === "uploading")) event.preventDefault();
  });
}
