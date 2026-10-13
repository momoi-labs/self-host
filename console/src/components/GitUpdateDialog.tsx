import { useEffect, useRef, useState } from "react";
import {
  Button, Dialog, DialogBody, DialogContent, DialogDescription,
  DialogFooter, DialogHeader, DialogTitle, FormField, KV, KVKey, KVValue, Skeleton, StatusBadge,
} from "@momoi-labs/kiso-react";

import { api, asReport, failureOf } from "../lib/api.js";
import { formatAgo } from "../lib/format.js";
import type { App, GitInspection, GitSource, Report } from "../lib/types.js";
import { Failure } from "./Failure.js";
import { Icon } from "./Icon.js";

/**
 * Reviews an update of a Git Application before building it: opening checks
 * the saved branch or tag and lists the commits since the deployed one, down
 * to it. When nothing moved it says so and offers to rebuild what is
 * deployed, so the main action always does something.
 */
export function GitUpdateDialog({ app, onClose, onDeploy, onRebuild }: {
  app: App;
  onClose: () => void;
  onDeploy: (source: GitSource, revision: string, expected: string) => Promise<void>;
  /** Builds the deployed commit again. */
  onRebuild: () => void;
}) {
  const saved = app.git?.git_ref ?? "HEAD";
  const [gitRef, setGitRef] = useState(saved);
  const [changingRef, setChangingRef] = useState(false);
  const [candidate, setCandidate] = useState<{ inspection: GitInspection; source: GitSource; expected: string } | null>(null);
  const [checking, setChecking] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const generation = useRef(0);
  const current = app.git_build?.revision;

  async function check(ref: string) {
    if (!app.git || !current) return;
    const request = ++generation.current;
    const expected = current;
    const source = { ...app.git, git_ref: ref.trim(), revision: null };
    setChecking(true);
    setCandidate(null);
    setFailure(null);
    try {
      const response = await api("/source/inspect", {
        method: "POST",
        body: JSON.stringify({ git: source, since: current }),
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

  // Opening is asking whether there is something new, so the check runs at once.
  useEffect(() => {
    void check(saved);
    return () => { ++generation.current; };
  }, []);

  const next = candidate?.inspection.revision;
  const upToDate = !!candidate && next === current;
  const history = candidate?.inspection.history;
  const commits = history?.commits ?? [];
  const ref = candidate?.source.git_ref ?? gitRef.trim();
  const close = () => { ++generation.current; onClose(); };

  return <Dialog open onOpenChange={(open) => { if (!open) close(); }}>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Update {app.name}</DialogTitle>
        <DialogDescription>
          {checking ? `Checking ${ref} for new commits.`
            : upToDate ? `${ref} has no commits since the deployed one.`
              : candidate ? `${commits.length}${history?.complete ? "" : "+"} ${commits.length === 1 ? "commit" : "commits"} to deploy from ${ref}.`
                : "Check the saved repository, then build the selected version."}
        </DialogDescription>
      </DialogHeader>
      <DialogBody>
        <div className="stack">
          {checking ? <Skeleton className="update-skeleton" /> : null}
          {failure ? <Failure failure={failure} /> : null}
          {upToDate ? (
            <KV>
              <KVKey>Deployed</KVKey>
              <KVValue><span className="update-deployed"><code>{current?.slice(0, 12)}</code><StatusBadge tone="success">Up to date</StatusBadge></span></KVValue>
              <KVKey>Checked</KVKey><KVValue>just now</KVValue>
            </KV>
          ) : candidate ? (
            <ol className="commit-timeline" aria-label="Commits to deploy">
              {commits.map((commit) => (
                <li key={commit.revision}>
                  <code>{commit.revision.slice(0, 7)}</code>
                  <span>{commit.subject}</span>
                  <span className="muted">{commit.author} · {formatAgo(commit.date)}</span>
                </li>
              ))}
              {history && !history.complete ? <li className="is-more"><span className="muted">Older commits not shown</span></li> : null}
              <li className="is-deployed">
                <code>{current?.slice(0, 7)}</code>
                <span className="muted">Deployed now</span>
                <StatusBadge tone="success">Deployed</StatusBadge>
              </li>
            </ol>
          ) : null}
          {changingRef ? (
            <div className="update-ref">
              <FormField id="update-git-ref" label="Branch or tag" className="mono" value={gitRef} disabled={checking}
                hint="Check another branch or tag. Deploying it saves it in place of the current one."
                onChange={(event) => setGitRef(event.target.value)} />
              <Button size="sm" disabled={checking || !gitRef.trim()} onClick={() => void check(gitRef)}>Check</Button>
            </div>
          ) : null}
          {app.git?.revision ? <p className="muted">Updating follows {ref} and removes the saved commit pin.</p> : null}
          <p className="muted t-label">Uses the saved configuration. A failed build keeps the current release.</p>
        </div>
      </DialogBody>
      <DialogFooter>
        {changingRef ? null : (
          <Button size="sm" variant="ghost" className="update-change-ref" disabled={checking} onClick={() => setChangingRef(true)}>
            <Icon name="tag" />Change branch or tag
          </Button>
        )}
        {upToDate ? <>
          <Button size="sm" onClick={() => { close(); onRebuild(); }}>Rebuild {current?.slice(0, 7)}</Button>
          <Button size="sm" variant="primary" onClick={close}>Close</Button>
        </> : <>
          <Button size="sm" onClick={close}>Cancel</Button>
          <Button size="sm" variant="primary" disabled={!candidate || !next || !current || !app.git} onClick={() => {
            if (!candidate || !next) return;
            close();
            void onDeploy(candidate.source, next, candidate.expected);
          }}>{next ? `Build and deploy ${next.slice(0, 7)}` : "Build and deploy"}</Button>
        </>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
