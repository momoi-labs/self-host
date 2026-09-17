import { useRef, useState } from "react";
import {
  Button,
  CommandPalette,
  CommandPaletteEmpty,
  CommandPaletteInput,
  CommandPaletteItem,
  CommandPaletteList,
} from "@momoi-labs/kiso-react";

import { Icon } from "./Icon.js";

/** Choose a creation form without leaving the current screen to search for it. */
export function CreateResource({
  onDeploy,
  onCreateMachine,
}: {
  onDeploy: () => void;
  onCreateMachine: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const trigger = useRef<HTMLButtonElement>(null);
  const actions = [
    { label: "Deploy application", run: onDeploy },
    { label: "New virtual machine", run: onCreateMachine },
  ].filter((action) => action.label.toLowerCase().includes(query.trim().toLowerCase()));

  const changeOpen = (next: boolean) => {
    setOpen(next);
    if (next) setQuery("");
    else requestAnimationFrame(() => trigger.current?.focus());
  };

  return (
    <>
      <Button
        ref={trigger}
        variant="primary"
        size="sm"
        className="btn-block"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => changeOpen(true)}
      >
        <Icon name="plus" />
        Create resource
      </Button>
      <CommandPalette label="Create resource" open={open} onOpenChange={changeOpen}>
        <CommandPaletteInput
          aria-label="Find a creation action"
          placeholder="What would you like to create?"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <CommandPaletteList>
          {actions.map((action) => (
            <CommandPaletteItem
              key={action.label}
              onSelect={() => {
                changeOpen(false);
                action.run();
              }}
            >
              {action.label}
            </CommandPaletteItem>
          ))}
          {actions.length === 0 && <CommandPaletteEmpty>No matching actions</CommandPaletteEmpty>}
        </CommandPaletteList>
      </CommandPalette>
    </>
  );
}
