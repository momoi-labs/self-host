import { useRef, useState } from "react";
import {
  Button, Dialog, DialogBody, DialogContent, DialogDescription,
  DialogFooter, DialogHeader, DialogTitle, FormField,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import type { App, GitInspection, GitSource, Report } from "../lib/types.js";
import { Failure } from "./Failure.js";

export function GitUpdateDialog({ app, onClose, onDeploy }: {
  app: App;
  onClose: () => void;
  onDeploy: (source: GitSource, revision: string, expected: string) => Promise<void>;
}) {
  const [gitRef, setGitRef] = useState(app.git?.git_ref ?? "HEAD");
  const [candidate, setCandidate] = useState<{ inspection: GitInspection; source: GitSource; expected: string } | null>(null);
  const [checking, setChecking] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const generation = useRef(0);
  const current = app.git_build?.revision;

  async function check() {
    if (!app.git || !current || checking) return;
    const request = ++generation.current;
    const expected = current;
    const source = { ...app.git, git_ref: gitRef.trim(), revision: null };
    setChecking(true);
    setCandidate(null);
    setFailure(null);
    try {
      const response = await api("/source/inspect", {
        method: "POST",
        body: JSON.stringify({ git: source }),
      });
      if (!response.ok) throw await failureOf(response);
      const result: GitInspection = await response.json();
      if (request === generation.current) setCandidate({ inspection: result, source, expected });
    } catch (cause) {
      if (request === generation.current) setFailure(asReport(cause));
    } finally {
      if (request === generation.current) setChecking(false);
    }
  }

  return <Dialog open onOpenChange={(open) => { if (!open) { ++generation.current; onClose(); } }}>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Update application</DialogTitle>
        <DialogDescription>Check the saved repository, then build the selected version.</DialogDescription>
      </DialogHeader>
      <DialogBody>
        <div className="stack">
          <FormField label="Branch or tag" id="update-git-ref">
            <input id="update-git-ref" value={gitRef} disabled={checking} onChange={(event) => {
              ++generation.current;
              setGitRef(event.target.value);
              setCandidate(null);
              setFailure(null);
            }} />
          </FormField>
          <dl className="summary-facts">
            <dt>Current commit</dt><dd><code>{current ?? "No successful build yet"}</code></dd>
            {candidate ? <><dt>Next commit</dt><dd><code>{candidate.inspection.revision}</code></dd></> : null}
          </dl>
          {app.git?.revision ? <p className="muted">Updating follows this branch or tag and removes the saved commit pin.</p> : null}
          {candidate?.inspection.revision === current ? <p role="status">This version is already deployed. Use Rebuild current version to build it again.</p> : null}
          {failure ? <Failure failure={failure} /> : null}
          <p className="muted">Uses saved configuration. A failed build keeps the current release.</p>
        </div>
      </DialogBody>
      <DialogFooter>
        <Button size="sm" onClick={onClose}>Cancel</Button>
        {candidate && candidate.inspection.revision !== current && current && app.git ? <Button size="sm" variant="primary" onClick={() => {
          onClose();
          void onDeploy(candidate.source, candidate.inspection.revision, candidate.expected);
        }}>Build and deploy update</Button> : <Button size="sm" variant="primary" disabled={checking || !gitRef.trim() || !current} onClick={() => void check()}>{checking ? "Checking..." : "Check for updates"}</Button>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
