import { useEffect, useState } from "react";

import { authHeaders } from "./api.js";

export type LogLine = {
  text: string;
  level?: "info" | "error";
};

/**
 * Container output, as the Platform streams it. Opening a stream replaces the
 * previous one: a redeploy replaces the container, and output from the old one
 * must not keep scrolling past.
 */
export function useLogStream(url: string | null): LogLine[] {
  const [lines, setLines] = useState<LogLine[]>([]);

  useEffect(() => {
    setLines([]);
    if (!url) return;

    const abort = new AbortController();

    void (async () => {
      try {
        const res = await fetch(url, { headers: authHeaders(), signal: abort.signal });
        if (!res.ok || !res.body) {
          setLines([{ text: "Could not connect to the log stream.", level: "error" }]);
          return;
        }

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        let event = "message";

        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });
          const chunk = buffer.split("\n");
          buffer = chunk.pop() ?? "";

          const batch: LogLine[] = [];
          for (const line of chunk) {
            // SSE format: "event: <name>" followed by "data: <content>".
            if (line.startsWith("event: ")) {
              event = line.slice(7);
              continue;
            }
            if (line === "") continue;
            const data = line.startsWith("data: ") ? line.slice(6) : line;
            if (data === "keepalive" || data === "") continue;
            batch.push({ text: data, level: event === "notice" ? "info" : undefined });
            event = "message";
          }
          if (batch.length) setLines((current) => [...current, ...batch]);
        }
      } catch (cause) {
        if ((cause as Error).name === "AbortError") return;
        setLines((current) => [...current, { text: "Connection lost", level: "error" }]);
      }
    })();

    return () => abort.abort();
  }, [url]);

  return lines;
}
