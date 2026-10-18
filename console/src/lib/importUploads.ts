import { useSyncExternalStore } from "react";

import { apiKey } from "./api.js";
import type { ArchiveFormat } from "./archiveFormat.js";
import { fileFingerprint } from "./fileFingerprint.js";

/**
 * Database import uploads, kept outside any screen so closing the import
 * dialog or moving to another console page does not stop one. The browser
 * sends the bytes, so a refresh or a closed tab does; the page warns first.
 * The server keeps what arrived, so picking the same file again resumes it.
 */
export type ImportUpload = {
  key: string;
  databaseId: string;
  connectionId: string;
  file: File;
  format: ArchiveFormat;
  /** One transaction that stops at the first error. */
  allOrNothing: boolean;
  /** The server's session for it, once started. */
  sessionId: string | null;
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

const CHUNK = 8 * 1024 * 1024;

/** Why the server refused a request, from its error report. */
function refusal(request: XMLHttpRequest): string {
  try {
    const error = (JSON.parse(request.responseText) as { error?: string }).error;
    if (error) return error;
  } catch { /* not JSON */ }
  return `The server refused the upload (${request.status})`;
}

/** One request to the upload API; `onProgress` gets the body bytes sent. */
function call(method: string, path: string, body: Blob | string | null, headers: Record<string, string>, onProgress?: (sent: number) => void): Promise<XMLHttpRequest> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open(method, path);
    request.setRequestHeader("Authorization", `Bearer ${apiKey()}`);
    for (const [name, value] of Object.entries(headers)) request.setRequestHeader(name, value);
    if (onProgress) request.upload.onprogress = (event) => onProgress(event.loaded);
    request.onload = () => resolve(request);
    request.onerror = () => reject(new Error("The connection dropped during the upload"));
    request.send(body);
  });
}

type Held = { id: string; size: number; received: number };

/**
 * Sends `upload` in chunks to a session on the server. The session is found
 * again by the file's fingerprint, so a retry, or the same file picked after
 * a refresh, sends only the bytes the server does not hold yet.
 */
async function send(upload: ImportUpload) {
  const fail = (error: string) => update(upload.key, { status: "failed", error });
  try {
    const base = `/databases/${upload.databaseId}/imports/uploads`;
    // A retry goes back to its own session: if that one was already
    // imported and only the answer got lost, it answers the same import.
    const known = upload.sessionId ? await call("GET", `${base}/${upload.sessionId}`, null, {}) : null;
    const started = known?.status === 200 ? known : await call("POST", base, JSON.stringify({
      connection_id: upload.connectionId,
      name: upload.file.name,
      size: upload.file.size,
      fingerprint: await fileFingerprint(upload.file),
      format: upload.format,
      all_or_nothing: upload.allOrNothing,
    }), { "Content-Type": "application/json" });
    if (started.status !== 200) return fail(refusal(started));
    const session = JSON.parse(started.responseText) as Held;
    let offset = session.received;
    // An older failed row for the same session is this upload now.
    publish(uploads.filter((item) => item.key === upload.key || item.sessionId !== session.id || item.status === "uploading"));
    update(upload.key, { sessionId: session.id, loaded: offset });
    while (offset < upload.file.size) {
      const from = offset;
      const end = Math.min(from + CHUNK, upload.file.size);
      const sent = await call("PUT", `${base}/${session.id}`, upload.file.slice(from, end), {
        "Content-Type": "application/octet-stream",
        "Content-Range": `bytes ${from}-${end - 1}/${upload.file.size}`,
      }, (bytes) => update(upload.key, { loaded: from + bytes }));
      // A 409 names the offset the server holds; the next chunk starts there.
      if (sent.status !== 200 && sent.status !== 409) return fail(refusal(sent));
      const held = (JSON.parse(sent.responseText) as Partial<Held>).received;
      if (typeof held !== "number") return fail(refusal(sent));
      offset = held;
      update(upload.key, { loaded: offset });
    }
    const finished = await call("POST", `${base}/${session.id}/import`, null, {});
    if (finished.status < 200 || finished.status >= 300) return fail(refusal(finished));
    const accepted = JSON.parse(finished.responseText) as { task_id: string };
    publish(uploads.filter((item) => item.key !== upload.key));
    for (const listener of finishedListeners) listener({ databaseId: upload.databaseId, taskId: accepted.task_id });
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error));
  }
}

/** Starts sending `file` and answers its key; progress shows in `useImportUploads`. */
export function startUpload(databaseId: string, connectionId: string, file: File, format: ArchiveFormat, allOrNothing: boolean): string {
  const upload: ImportUpload = {
    key: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    databaseId, connectionId, file, format, allOrNothing, sessionId: null, loaded: 0, status: "uploading", error: null,
  };
  publish([...uploads, upload]);
  void send(upload);
  return upload.key;
}

/** Sends a failed upload again; it goes on from the bytes the server holds. */
export function retryUpload(key: string) {
  const upload = uploads.find((item) => item.key === key);
  if (!upload || upload.status !== "failed") return;
  const again = { ...upload, status: "uploading" as const, error: null };
  publish(uploads.map((item) => (item.key === key ? again : item)));
  void send(again);
}

/** Forgets a failed upload, and the bytes the server holds for it. */
export function dismissUpload(key: string) {
  const upload = uploads.find((item) => item.key === key);
  if (!upload || upload.status === "uploading") return;
  // Another row may have found the same session; it keeps the bytes.
  const shared = uploads.some((item) => item.key !== key && item.sessionId === upload.sessionId);
  if (upload.sessionId && !shared) void call("DELETE", `/databases/${upload.databaseId}/imports/uploads/${upload.sessionId}`, null, {}).catch(() => undefined);
  publish(uploads.filter((item) => item.key !== key));
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
