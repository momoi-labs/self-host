import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { DiagramNode } from "@momoi-labs/kiso-react";

import { readJson } from "../lib/api.js";
import { formatBytes } from "../lib/format.js";
import { Icon } from "./Icon.js";

/** One process as the Host lists it; `ppid` not in the list makes it a root of the tree. */
type Process = { pid: number; ppid: number; user: string; command: string; cpu_percent?: number; rss_bytes?: number };
/** A container's processes, or a native Application's whole tree under the name `process`. */
type ProcessGroup = { name: string; processes: Process[] };
type Node = { process: Process; children: Node[]; depth: number; last: boolean };

/** Roots are the processes whose parent is not listed: what s6 or the container started, and any terminal. */
function treeOf(processes: Process[]): Node[] {
  const pids = new Set(processes.map((process) => process.pid));
  const build = (list: Process[], depth: number): Node[] => list.map((process, index) => ({
    process, depth, last: index === list.length - 1,
    children: build(processes.filter((child) => child.ppid === process.pid), depth + 1),
  }));
  return build(processes.filter((process) => !pids.has(process.ppid)), 0);
}

function flatten(nodes: Node[]): Node[] {
  return nodes.flatMap((node) => [node, ...flatten(node.children)]);
}

const cpuOf = (processes: Process[]) => `${processes.reduce((sum, p) => sum + (p.cpu_percent ?? 0), 0).toFixed(1)}%`;
const memoryOf = (processes: Process[]) => formatBytes(processes.reduce((sum, p) => sum + (p.rss_bytes ?? 0), 0));

/**
 * What an Application runs right now, on a card under the node that
 * receives requests. Folded, the header says how many processes and what
 * they use together; open, it is the tree as a table, with a chip per
 * container. It reads again every few seconds while the Summary is open.
 */
export function ProcessCard({ id, native }: { id: string; native: boolean }) {
  const [open, setOpen] = useState(false);
  const [only, setOnly] = useState<string | null>(null);
  const groups = useQuery({
    queryKey: ["processes", id],
    queryFn: ({ signal }) => readJson<ProcessGroup[]>(`/apps/id/${id}/processes`, signal),
    refetchInterval: 5_000,
    retry: false,
  }).data ?? [];
  const all = groups.flatMap((group) => group.processes);
  // A Host that cannot list them, or an Application with nothing running, has no card.
  if (!all.length) return null;
  const many = groups.length > 1;
  const roots = groups.reduce((count, group) => count + treeOf(group.processes).length, 0);
  const running = groups.filter((group) => group.processes.length).length;
  const shown = many && only ? groups.filter((group) => group.name === only) : groups;
  // A Compose container is named after its Application and service; the service is the part that tells them apart.
  const label = (name: string) => name.replace(`sf-app-${id}-`, "");

  return (
    <DiagramNode id="processes" title="" className="group-node card-rest process-card">
      <span className="group-card">
        <button type="button" className="group-head process-head" aria-expanded={open} onClick={() => setOpen(!open)}>
          <Icon name={native ? "terminal" : "box"} />
          <span className="process-count">
            {all.length} {all.length === 1 ? "process" : "processes"} · {many ? `${running} of ${groups.length} containers running` : `${roots} ${roots === 1 ? "root" : "roots"}`}
          </span>
          <span className="mono process-stat process-stats" title="CPU"><Icon name="cpu" />{cpuOf(all)}</span>
          <span className="mono process-stat" title="Memory"><Icon name="memory" />{memoryOf(all)}</span>
          <Icon name={open ? "chevron-up" : "chevron-down"} />
        </button>
        <span className="group-body" data-open={open} aria-hidden={!open}>
          {many ? (
            <span className="group-chips" role="group" aria-label="Show">
              <button type="button" aria-pressed={!only} onClick={() => setOnly(null)}>All {all.length}</button>
              {groups.map((group) => (
                <button key={group.name} type="button" aria-pressed={only === group.name} onClick={() => setOnly(group.name)}>
                  {label(group.name)} {group.processes.length}
                </button>
              ))}
            </span>
          ) : null}
          <span className="process-scroll">
          <table className="process-table">
            <thead>
              <tr><th scope="col">Command</th><th scope="col" className="num">PID</th><th scope="col" className="num">CPU</th><th scope="col" className="num">Memory</th></tr>
            </thead>
            <tbody>
              {shown.flatMap((group) => [
                many ? (
                  <tr key={`${group.name}:head`} className="process-group">
                    <td colSpan={4}><Icon name="box" /> {label(group.name)}{group.processes.length ? null : <span className="muted"> · not running</span>}</td>
                  </tr>
                ) : null,
                ...flatten(treeOf(group.processes)).map((node) => (
                  <tr key={`${group.name}:${node.process.pid}`}>
                    <td title={node.process.command}>
                      <span className="process-command" style={{ paddingInlineStart: `${node.depth * 1.25}rem` }}>
                        {node.depth ? <span className="process-guide" aria-hidden="true">{node.last ? "└─" : "├─"}</span> : null}
                        <span className="mono">{node.process.command}</span>
                      </span>
                    </td>
                    <td className="num mono muted">{node.process.pid}</td>
                    <td className="num mono">{node.process.cpu_percent === undefined ? "–" : `${node.process.cpu_percent.toFixed(1)}%`}</td>
                    <td className="num mono">{node.process.rss_bytes === undefined ? "–" : formatBytes(node.process.rss_bytes)}</td>
                  </tr>
                )),
              ])}
            </tbody>
          </table>
          </span>
        </span>
      </span>
    </DiagramNode>
  );
}
