import { useEffect, useRef, type ComponentProps } from "react";
import { Textarea } from "@momoi-labs/kiso-react";

declare global {
  interface Window {
    Prism?: {
      languages: Record<string, unknown>;
      highlight: (code: string, grammar: unknown, language: string) => string;
    };
  }
}

/**
 * The textarea keeps native editing; the layer underneath only paints YAML.
 * Prism is loaded as a page script rather than bundled, so a console without
 * it still edits — it just edits in one colour.
 */
export function ComposeEditor({
  id,
  value,
  onChange,
  required,
  "aria-describedby": describedBy,
  "aria-invalid": invalid,
}: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  required?: boolean;
  "aria-describedby"?: string;
  "aria-invalid"?: ComponentProps<typeof Textarea>["aria-invalid"];
}) {
  const textarea = useRef<HTMLTextAreaElement>(null);
  const highlight = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const layer = highlight.current;
    const prism = window.Prism;
    if (!layer) return;
    if (!prism?.languages.yaml) {
      layer.textContent = "";
      return;
    }
    layer.innerHTML = prism.highlight(value + "\n", prism.languages.yaml, "yaml");
  }, [value]);

  const syncScroll = () => {
    const layer = highlight.current;
    const field = textarea.current;
    if (!layer || !field) return;
    layer.scrollTop = field.scrollTop;
    layer.scrollLeft = field.scrollLeft;
  };

  return (
    <div className="yaml-editor">
      <div className="yaml-highlight" ref={highlight} aria-hidden="true" />
      <Textarea
        ref={textarea}
        className="mono compose-editor"
        id={id}
        rows={14}
        wrap="off"
        spellCheck={false}
        placeholder={"services:\n  web:\n    image: …"}
        required={required}
        aria-describedby={describedBy}
        aria-invalid={invalid}
        value={value}
        onChange={(event) => {
          onChange(event.target.value);
          syncScroll();
        }}
        onScroll={syncScroll}
      />
    </div>
  );
}
