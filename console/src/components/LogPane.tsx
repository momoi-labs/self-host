import { useEffect, useState } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { getJson } from "../lib/api.js";
import { Field } from "./Field.js";
import { Logs } from "./Logs.js";

/**
 * The log pane of an Application. One container needs no choosing, so it says
 * which one instead; several get a picker, and switching replaces the stream.
 */
export function AppLogPane({ id }: { id: string }) {
  const [containers, setContainers] = useState<string[] | null>(null);
  const [chosen, setChosen] = useState("");
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setContainers(null);
    setChosen("");
    setFailed(false);
    void (async () => {
      const names = await getJson<string[]>(`/apps/id/${encodeURIComponent(id)}/containers`);
      if (cancelled) return;
      if (!names) {
        setFailed(true);
        return;
      }
      setContainers(names);
      setChosen(names[0] ?? "");
    })();
    return () => {
      cancelled = true;
    };
  }, [id]);

  if (failed) {
    return (
      <>
        <p className="t-caps">Logs</p>
        <div className="logview">
          <div className="log-scroll log-error">
            Could not load containers. Reopen the application to retry.
          </div>
        </div>
      </>
    );
  }

  return (
    <>
      {containers && containers.length > 1 ? (
        <Field id="log-container" label="Logs from container">
          <Select value={chosen} onValueChange={setChosen}>
            <SelectTrigger id="log-container" className="mono">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {containers.map((name) => (
                <SelectItem key={name} value={name}>
                  {name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      ) : (
        <p className="t-caps">{chosen ? `Logs from ${chosen}` : "Logs"}</p>
      )}

      <Logs
        label="Logs"
        url={
          chosen
            ? `/apps/id/${encodeURIComponent(id)}/logs?container=${encodeURIComponent(chosen)}`
            : null
        }
      />
    </>
  );
}
