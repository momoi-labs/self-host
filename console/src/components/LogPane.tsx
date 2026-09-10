import { useEffect, useState } from "react";
import {
  FormField,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { getJson } from "../lib/api.js";
import { Logs } from "./Logs.js";

/**
 * Multiple containers get a picker; switching replaces the stream.
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
        <Select value={chosen} onValueChange={setChosen}>
          <FormField id="log-container" label="Logs from container">
            <SelectTrigger className="mono">
              <SelectValue />
            </SelectTrigger>
          </FormField>
          <SelectContent>
            {containers.map((name) => (
              <SelectItem key={name} value={name}>
                {name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : (
        <p className="t-caps">Logs</p>
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
