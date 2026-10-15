import { useState } from "react";
import {
  Chip,
  ChipInput,
  ChipInputBox,
  ChipInputEmpty,
  ChipInputField,
  ChipInputList,
  ChipInputOption,
  ChipName,
  ChipRemove,
  Label,
} from "@momoi-labs/kiso-react";

import type { App } from "../lib/types.js";
import { grantable } from "../lib/privateAccess.js";

/**
 * The Applications this unpublished Compose Application lets in. Each one
 * joins its private network and reaches its services by container name.
 * Saved with the rest of the form.
 */
export function PrivateAccessField({ app, apps, value, onChange, disabled }: {
  app: App;
  apps: App[];
  /** Consumer Application ids, in the order they were granted. */
  value: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [focused, setFocused] = useState(false);
  const byId = new Map(apps.map((one) => [one.id, one]));
  const typed = query.trim().toLowerCase();
  const choices = grantable(app, apps)
    .filter((one) => !value.includes(one.id))
    .filter((one) => !typed || one.name.toLowerCase().includes(typed));
  const add = (id: string) => {
    onChange([...value, id]);
    setQuery("");
  };

  return (
    <div className="field">
      <Label htmlFor="f-private-access">Applications with access</Label>
      <ChipInput>
        <ChipInputBox
          disabled={disabled}
          onFocus={() => setFocused(true)}
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false);
          }}
        >
          {value.map((id) => {
            // A grant can outlive the name it was given under; the id still says which.
            const name = byId.get(id)?.name ?? id;
            return (
              <Chip key={id}>
                <ChipName>{name}</ChipName>
                <ChipRemove aria-label={`Remove access for ${name}`} disabled={disabled}
                  onClick={() => onChange(value.filter((item) => item !== id))} />
              </Chip>
            );
          })}
          <ChipInputField
            id="f-private-access"
            aria-describedby="f-private-access-help"
            placeholder={value.length ? "" : "Pick an Application"}
            disabled={disabled}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onRemoveLast={() => onChange(value.slice(0, -1))}
            onKeyDown={(event) => {
              if (event.defaultPrevented || event.key !== "Enter") return;
              event.preventDefault();
              if (choices.length === 1) add(choices[0].id);
            }}
          />
        </ChipInputBox>
        {focused ? (
          <ChipInputList aria-label="Applications">
            {choices.map((one) => (
              <ChipInputOption key={one.id} onSelect={() => add(one.id)}>
                <span>{one.name}</span>
              </ChipInputOption>
            ))}
            {!choices.length ? (
              <ChipInputEmpty>{typed ? "No Application by that name." : "Every container Application already has access."}</ChipInputEmpty>
            ) : null}
          </ChipInputList>
        ) : null}
      </ChipInput>
      <small className="field-hint" id="f-private-access-help">
        Each one joins this Application's private network and reaches a service by its container
        name, shown in the Summary. Nothing else can reach it. Saving redeploys this Application.
      </small>
    </div>
  );
}
