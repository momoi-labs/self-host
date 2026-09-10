import { useEffect, useState } from "react";

import { authHeaders } from "./api.js";
import { logEventParser, type LogLine } from "./logEvents.js";

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
        const parse = logEventParser();

        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;

          const batch = parse(decoder.decode(value, { stream: true }));
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
