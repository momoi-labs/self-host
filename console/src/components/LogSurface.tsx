import { LogView, LogViewLine } from "@momoi-labs/kiso-react";

/**
 * A log that is the whole panel: flush with its edges, filling its height, and
 * scrolling itself. The image builder established the surface and the
 * machine screen wants the same one, so the rendering lives here instead of in
 * both.
 *
 * A log arrives as one blob of text and its blank lines have to stay blank, or
 * the spacing the guest printed collapses.
 */
export function LogSurface({
  text,
  placeholder,
  label,
}: {
  text: string | null | undefined;
  placeholder: string;
  label: string;
}) {
  return (
    <LogView
      className="logview-flush"
      role="log"
      aria-live="polite"
      tabIndex={0}
      aria-label={label}
    >
      {(text || placeholder).split("\n").map((line, index) => (
        <LogViewLine key={index}>{line || " "}</LogViewLine>
      ))}
    </LogView>
  );
}
