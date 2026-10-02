import { useEffect, useRef, useState } from "react";
import {
  Button,
  Search,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { listGitRepositories, useGitConnections, type GitRepository } from "../lib/gitConnections.js";
import type { Report } from "../lib/types.js";
import { Failure } from "./Failure.js";

export type { GitRepository } from "../lib/gitConnections.js";

export function GitRepositoryPicker({ credentialId, disabled, onImport }: {
  credentialId: string;
  disabled?: boolean;
  onImport: (repository: GitRepository) => void;
}) {
  const { connections, loading: connectionsLoading, error: connectionsError } = useGitConnections();
  const connection = connections.find((item) => item.credential_id === credentialId);
  const [repositories, setRepositories] = useState<GitRepository[]>([]);
  const [nextPage, setNextPage] = useState<number | null>(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [failure, setFailure] = useState<Report | null>(null);
  const [retry, setRetry] = useState(0);
  const request = useRef(0);

  useEffect(() => {
    const generation = ++request.current;
    setRepositories([]);
    setNextPage(null);
    setLoaded(false);
    setFailure(null);
    setQuery("");
    if (!connection || connection.status !== "connected") {
      setLoading(false);
      return;
    }
    setLoading(true);
    void listGitRepositories(connection.id).then((page) => {
      if (request.current !== generation) return;
      setRepositories(page.repositories);
      setNextPage(page.next_page ?? null);
      setLoaded(true);
    }).catch((cause) => {
      if (request.current === generation) setFailure(asReport(cause));
    }).finally(() => {
      if (request.current === generation) setLoading(false);
    });
    return () => { ++request.current; };
  }, [connection?.id, connection?.status, retry]);

  async function loadMore() {
    if (!connection || nextPage === null || loading) return;
    const generation = request.current;
    setLoading(true);
    setFailure(null);
    try {
      const page = await listGitRepositories(connection.id, nextPage);
      if (request.current !== generation) return;
      setRepositories((current) => {
        const urls = new Set(current.map((repository) => repository.clone_url));
        return [...current, ...page.repositories.filter((repository) => !urls.has(repository.clone_url))];
      });
      setNextPage(page.next_page ?? null);
    } catch (cause) {
      if (request.current === generation) setFailure(asReport(cause));
    } finally {
      if (request.current === generation) setLoading(false);
    }
  }

  if (connectionsError) return <Failure failure={connectionsError} />;
  if (connectionsLoading && !connection) return <p className="muted" role="status">Loading Git connections...</p>;
  if (!connection) return <p className="muted">Select a connection to list repositories, or use a repository URL.</p>;
  if (connection.status !== "connected") return <p className="muted">This connection expired. Reconnect it before importing a repository.</p>;

  const visible = repositories.filter((repository) => repository.full_name.toLowerCase().includes(query.trim().toLowerCase()));

  return <section className="git-repository-picker" aria-label="Import a repository">
    <div className="git-repository-heading">
      <div><h2>Import a repository</h2><p className="muted">{connection.name}</p></div>
      <Search aria-label="Search repositories" placeholder="Search repositories..." value={query} onChange={(event) => setQuery(event.target.value)} />
    </div>
    {failure ? <Failure failure={failure} actionLabel={loaded ? undefined : "Retry"} onAction={() => setRetry((value) => value + 1)} /> : null}
    {!loaded && loading ? <p className="muted" role="status">Loading repositories...</p> : loaded ? <div className="table-wrap">
      <div className="table-scroll">
        <Table>
          <TableHeader><TableRow><TableHead scope="col">Repository</TableHead><TableHead scope="col">Access</TableHead><TableHead scope="col">Action</TableHead></TableRow></TableHeader>
          <TableBody>
            {!visible.length ? <TableRow><TableCell colSpan={3} className="muted">{repositories.length ? "No repositories match your search." : "This connection has no accessible repositories."}</TableCell></TableRow> : visible.map((repository) => <TableRow key={repository.clone_url}>
              <TableCell><strong>{repository.full_name}</strong></TableCell>
              <TableCell>{repository.private ? "Private" : "Public"}</TableCell>
              <TableCell><Button type="button" size="sm" disabled={disabled} aria-label={`Import ${repository.full_name}`} onClick={() => onImport(repository)}>Import</Button></TableCell>
            </TableRow>)}
          </TableBody>
        </Table>
      </div>
      <p className="table-footer"><span>{visible.length} of {repositories.length} loaded repositories</span></p>
    </div> : null}
    {nextPage !== null ? <Button type="button" size="sm" disabled={loading || disabled} onClick={() => void loadMore()}>{loading ? "Loading..." : "Load more repositories"}</Button> : null}
  </section>;
}
