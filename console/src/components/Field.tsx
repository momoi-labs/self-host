import type { ReactNode } from "react";
import { Label, ValidationMessage } from "@momoi-labs/kiso-react";

/**
 * kiso's FormField pairs a label with an Input. This is the same field with
 * room for a refusal underneath, and for a control that is not an Input.
 */
export function Field({
  id,
  label,
  hint,
  error,
  className,
  children,
}: {
  id: string;
  label: string;
  hint?: ReactNode;
  error?: string | null;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={className ? `field ${className}` : "field"}>
      <Label htmlFor={id}>{label}</Label>
      {children}
      {hint ? (
        <small className="field-hint" id={`${id}-help`}>
          {hint}
        </small>
      ) : null}
      {error ? <ValidationMessage id={`${id}-error`}>{error}</ValidationMessage> : null}
    </div>
  );
}

/** What a control points at, given which of its descriptions exist. */
export function describedBy(id: string, hint: boolean, error: boolean): string | undefined {
  return [hint ? `${id}-help` : null, error ? `${id}-error` : null].filter(Boolean).join(" ") || undefined;
}
