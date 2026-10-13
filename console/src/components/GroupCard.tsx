import { useState, type ReactNode } from "react";

import { Icon, type IconName } from "./Icon.js";

/** One line of a section: what it is, and where it goes. Its section's heading carries the icon. */
/** `env` names the environment variable the line arrives in, drawn as a chip in place of `to`. */
export type GroupLine = { key: string; label: string; to?: string; env?: string; hint?: string; onOpen?: () => void };

/** A kind of thing the card lists, with the way to add one: a dialog, or a page that does it. */
/** A `flat` section has no heading of its own: its lines sit straight under the card's header. */
export type GroupSection = { key: string; title: string; icon: IconName; lines: GroupLine[]; onAdd?: () => void; addHref?: string; flat?: boolean };

/**
 * A card that lists what one side of the Summary diagram holds, in the shape
 * of Cloudflare's: a header with the count that folds the card, then a
 * section per kind with its count and add action, its lines hanging from it
 * as a tree. Once more than one kind is there, chips show one at a time.
 * It sits in a kiso diagram node with `className="group-node"`.
 */
export function GroupCard({ title, sections, facts, count }: {
  title: string;
  sections: GroupSection[];
  /** Single facts drawn above the sections, such as the one Hostname; they are not filtered. */
  facts?: ReactNode;
  /** The header's count when it covers more than the sections' lines. */
  count?: number;
}) {
  const [open, setOpen] = useState(true);
  const [only, setOnly] = useState<string | null>(null);
  const present = sections.filter((section) => section.lines.length > 0);
  const total = present.reduce((count, section) => count + section.lines.length, 0);
  // A filter that has nothing left to show falls back to all of them.
  const shown = present.length > 1 && present.some((section) => section.key === only)
    ? present.filter((section) => section.key === only)
    : present;
  const add = (section: GroupSection): ReactNode => {
    const label = `Add to ${section.title}`;
    if (section.addHref) return <a className="group-add" href={section.addHref} aria-label={label}><Icon name="plus" /></a>;
    return section.onAdd ? <button type="button" className="group-add" aria-label={label} onClick={section.onAdd}><Icon name="plus" /></button> : null;
  };

  return (
    <span className="group-card">
      <button type="button" className="group-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span>{title} <span className="group-count">{count ?? total}</span></span>
        <Icon name={open ? "chevron-up" : "chevron-down"} />
      </button>
      {/* Folded, the body stays in the layout with no height, so the card keeps its width. */}
      <span className="group-body" data-open={open} aria-hidden={!open}>
      {facts ? <span className="group-section is-flat group-facts">{facts}</span> : null}
      {present.length > 1 ? (
        <span className="group-chips" role="group" aria-label="Show">
          <button type="button" aria-pressed={shown.length > 1} onClick={() => setOnly(null)}>All {total}</button>
          {present.map((section) => (
            <button key={section.key} type="button" aria-pressed={shown.length === 1 && shown[0].key === section.key} onClick={() => setOnly(section.key)}>
              {section.title} {section.lines.length}
            </button>
          ))}
        </span>
      ) : null}
      {shown.map((section) => (
        <span key={section.key} className={section.flat ? "group-section is-flat" : "group-section"}>
          {section.flat ? null : <span className="group-section-head">
            <span><Icon name={section.icon} />{section.title} <span className="group-count">{section.lines.length}</span></span>
            {add(section)}
          </span>}
          {section.lines.map((line) => {
            const body = <>
              <span className="group-name"><span className="mono">{line.label}</span></span>
              {line.env ? (
                <span className="group-env" title={line.hint}><b>env</b><span className="mono">{line.env}</span></span>
              ) : null}
              {line.to ? <span className="mono muted" title={line.env ? undefined : line.hint}>{line.to}</span> : null}
            </>;
            return line.onOpen
              ? <button key={line.key} type="button" className="group-line" onClick={line.onOpen}>{body}</button>
              : <span key={line.key} className="group-line">{body}</span>;
          })}
        </span>
      ))}
      </span>
    </span>
  );
}

/** One fact as a line: its icon and name, and the value on the right. */
export function Fact({ icon, label, value, empty }: { icon: IconName; label: string; value?: ReactNode; empty?: string }) {
  return (
    <span className="group-line">
      <span className="group-name"><Icon name={icon} /><span>{label}</span></span>
      <span className={value ? "mono fact-value" : "muted"}>{value ?? empty}</span>
    </span>
  );
}
