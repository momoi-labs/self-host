import { useState } from "react";
import {
  Checkbox,
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
import { grantable, isShared } from "../lib/privateAccess.js";

/**
 * The Applications this unpublished Compose Application lets in. Each one
 * joins its private network and reaches its services by container name.
 * Saved with the rest of the form.
 */
export function PrivateAccessField({ app, apps, value, onChange, makePrivate, onMakePrivate, disabled }: {
  app: App;
  apps: App[];
  /** Consumer Application ids, in the order they were granted. */
  value: string[];
  onChange: (next: string[]) => void;
  /** For an Application still on the shared network: turn it private on Save. */
  makePrivate: boolean;
  onMakePrivate: (next: boolean) => void;
  disabled?: boolean;
}) {
  const shared = isShared(app);
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
      {shared ? (
        // Rows from before private networks still join the common bridge.
        <div className="check">
          <Checkbox id="f-make-private" checked={makePrivate || value.length > 0} disabled={disabled || value.length > 0}
            onCheckedChange={(checked) => onMakePrivate(checked === true)} />
          <Label htmlFor="f-make-private">Only the Applications above can reach it</Label>
        </div>
      ) : null}
      <small className="field-hint" id="f-private-access-help">
        {shared && !makePrivate && !value.length
          ? "Today it is on the shared network, where every other shared Application reaches it. "
          : "Nothing else can reach it. "}
        Each Application above joins its private network and reaches a service by its container
        name, shown in the Summary. Saving redeploys this Application.
      </small>
    </div>
  );
}
