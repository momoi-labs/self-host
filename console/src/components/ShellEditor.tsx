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

/** Native shell editing with a Prism layer and a synced line-number gutter. */
export function ShellEditor({ id, value, onChange, disabled, maxLength, placeholder, "aria-describedby": describedBy }: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  maxLength?: number;
  placeholder?: string;
  "aria-describedby"?: ComponentProps<typeof Textarea>["aria-describedby"];
}) {
  const textarea = useRef<HTMLTextAreaElement>(null);
  const highlight = useRef<HTMLDivElement>(null);
  const gutter = useRef<HTMLDivElement>(null);
  const editor = useRef<HTMLDivElement>(null);
  const lineCount = Math.max(1, value.split("\n").length);

  useEffect(() => {
    const layer = highlight.current;
    const prism = window.Prism;
    if (!layer) return;
    if (!prism?.languages.bash) {
      layer.textContent = "";
      editor.current?.classList.add("shell-editor-fallback");
      return;
    }
    editor.current?.classList.remove("shell-editor-fallback");
    layer.innerHTML = prism.highlight(value + "\n", prism.languages.bash, "bash");
  }, [value]);

  const syncScroll = () => {
    const layer = highlight.current;
    const field = textarea.current;
    const numbers = gutter.current;
    if (!layer || !field || !numbers) return;
    layer.scrollTop = field.scrollTop;
    layer.scrollLeft = field.scrollLeft;
    numbers.scrollTop = field.scrollTop;
  };

  return (
    <div className="shell-editor" ref={editor}>
      <div className="shell-editor-gutter" ref={gutter} aria-hidden="true">
        {Array.from({ length: lineCount }, (_, index) => <span key={index}>{index + 1}</span>)}
      </div>
      <div className="shell-editor-code">
        <div className="shell-highlight" ref={highlight} aria-hidden="true" />
        <Textarea ref={textarea} className="mono shell-editor-input" id={id} rows={5} wrap="off"
          spellCheck={false} placeholder={placeholder} disabled={disabled} maxLength={maxLength}
          aria-describedby={describedBy} value={value}
          onChange={(event) => onChange(event.target.value)} onScroll={syncScroll} />
      </div>
    </div>
  );
}
