import type { ReactNode } from "react";

import { Icon, type IconName } from "./Icon.js";

export type SaveBarTone = "neutral" | "warning" | "accent" | "danger";

const icons: Record<SaveBarTone, IconName | null> = {
  neutral: null,
  warning: "alert",
  accent: "info",
  danger: "alert",
};

/**
 * The footer of every detail form: what state the record is in, and the one
 * or two things the Operator can do about it. It sticks to the bottom of
 * whatever scrolls, so a five-step form never hides its Save under the fold
 * and "you have not saved this" is read without scrolling to find out.
 *
 * The tone is the state. Neutral is nothing to say; warning is edits that are
 * not saved; accent is saved and waiting for the machine to take it; danger is
 * the last attempt failing. A screen with only a Save passes no message.
 */
export function SaveBar({
  tone = "neutral",
  message,
  actions,
}: {
  tone?: SaveBarTone;
  message?: ReactNode;
  actions?: ReactNode;
}) {
  const icon = icons[tone];
  return (
    <div className={`save-bar save-bar-${tone}`} role="status">
      <span className="save-bar-message">
        {icon ? <Icon name={icon} size="md" /> : null}
        {message}
      </span>
      {actions ? <span className="save-bar-actions">{actions}</span> : null}
    </div>
  );
}
