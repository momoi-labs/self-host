import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  FormField,
  LogView,
  LogViewLine,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { readJson } from "../lib/api.js";
import { Logs } from "./Logs.js";

/**
 * Multiple containers get a picker; switching replaces the stream.
 */
export function AppLogPane({ id, native = false }: { id: string; native?: boolean }) {
  const list = useQuery({
    queryKey: ["apps", id, "containers"],
    queryFn: ({ signal }) => readJson<string[]>(`/apps/id/${encodeURIComponent(id)}/containers`, signal),
    enabled: !native,
  });
  const containers = list.data ?? null;
  const [chosen, setChosen] = useState("");
  // The first container until the Operator picks one, and again if theirs is gone.
  const container = containers?.includes(chosen) ? chosen : (containers?.[0] ?? "");

  if (list.isError && !containers) {
    return (
      <LogView>
        <LogViewLine className="log-error">
          Could not load containers. Reopen the application to retry.
        </LogViewLine>
      </LogView>
    );
  }

  return (
    <>
      {/* The tab is already called Logs. Only a Compose Application with
          several containers needs anything above the stream, and then it is
          the picker. */}
      {containers && containers.length > 1 ? (
        <Select value={container} onValueChange={setChosen}>
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
      ) : null}

      <Logs
        label="Logs"
        url={
          native ? `/apps/id/${encodeURIComponent(id)}/logs` : container
            ? `/apps/id/${encodeURIComponent(id)}/logs?container=${encodeURIComponent(container)}`
            : null
        }
      />
    </>
  );
}
