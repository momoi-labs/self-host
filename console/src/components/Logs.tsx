import { LogView, LogViewLine } from "@momoi-labs/kiso-react";

import { useLogStream } from "../lib/useLogStream.js";

/** The log pane: a stream, followed to the tail while the operator stays there. */
export function Logs({ url, label }: { url: string | null; label: string }) {
  const lines = useLogStream(url);

  return (
    <LogView role="log" aria-live="polite" tabIndex={0} aria-label={label}>
      {lines.map((line, i) => (
        <LogViewLine key={i} className={/^[ ▀▄█]+$/.test(line.text) && /[▀▄█]/.test(line.text)
          ? "log-block-art" : line.level ? `log-${line.level}` : undefined}>
          {line.text}
        </LogViewLine>
      ))}
    </LogView>
  );
}
