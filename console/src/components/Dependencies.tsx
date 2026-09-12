import { useEffect, useMemo, useState } from "react";
import {
  Chip,
  ChipInput,
  ChipInputBox,
  ChipInputEmpty,
  ChipInputField,
  ChipInputList,
  ChipInputOption,
  ChipName,
  ChipOption,
  ChipOptionAdd,
  ChipRemove,
  ChipScope,
  ChipValue,
  Label,
  ValidationMessage,
} from "@momoi-labs/kiso-react";

import { api, failureOf } from "../lib/api.js";
import type { ImageDependency } from "../lib/devImageTemplates.js";
import {
  ALLOW_BUILDS,
  isKey,
  isVersion,
  readOption,
  splitKey,
  suggest,
  takesAllowBuilds,
} from "../lib/dependencies.js";

type MiseTool = { name: string; backends: string[] };
let toolCatalog: Promise<MiseTool[]> | null = null;

function loadToolCatalog(): Promise<MiseTool[]> {
  if (!toolCatalog)
    toolCatalog = (async () => {
      const response = await api("/dev-images/tools");
      if (!response.ok) throw await failureOf(response);
      const payload: unknown = await response.json();
      if (!Array.isArray(payload))
        throw new Error("Invalid tool search response");
      return payload.flatMap((entry: unknown): MiseTool[] => {
        if (typeof entry === "string") return [{ name: entry, backends: [] }];
        if (
          !entry ||
          typeof entry !== "object" ||
          !("name" in entry) ||
          typeof entry.name !== "string"
        )
          return [];
        return [
          {
            name: entry.name,
            backends:
              "backends" in entry && Array.isArray(entry.backends)
                ? entry.backends.filter(
                    (key): key is string => typeof key === "string",
                  )
                : [],
          },
        ];
      });
    })().catch((error) => {
      toolCatalog = null;
      throw error;
    });
  return toolCatalog;
}

/** Shared catalog-backed mise dependency editor for images and environments. */
export function Dependencies({
  value,
  onChange,
  disabled,
  id = "dependency-search",
}: {
  value: ImageDependency[];
  onChange: (value: ImageDependency[]) => void;
  disabled: boolean;
  id?: string;
}) {
  const [tools, setTools] = useState<MiseTool[]>([]);
  const [loading, setLoading] = useState(false);
  const [searchFailed, setSearchFailed] = useState(false);
  const [query, setQuery] = useState("");
  const [focused, setFocused] = useState(false);
  const typed = query.trim();
  const matches = suggest(
    tools,
    query,
    value.map((dep) => dep.tool),
  );
  const known = useMemo(
    () => new Set(tools.flatMap((tool) => [tool.name, ...tool.backends])),
    [tools],
  );
  const unversioned = value.filter((dep) => !isVersion(dep.version));

  useEffect(() => {
    let active = true;
    setLoading(true);
    void loadToolCatalog()
      .then((results) => {
        if (active) setTools(results);
      })
      .catch(() => {
        if (active) setSearchFailed(true);
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, []);

  function add(tool: string) {
    setQuery("");
    if (!value.some((dep) => dep.tool === tool))
      onChange([...value, { tool, version: "latest" }]);
  }
  function update(tool: string, change: Partial<ImageDependency>) {
    onChange(
      value.map((dep) => (dep.tool === tool ? { ...dep, ...change } : dep)),
    );
  }
  function setOption(tool: string, text: string) {
    const option = readOption(text);
    if (option?.name === ALLOW_BUILDS)
      update(tool, {
        allow_builds: option.values.length ? option.values : undefined,
      });
  }

  return (
    <div className="field">
      <Label htmlFor={id}>Dependencies</Label>
      <ChipInput>
        <ChipInputBox
          disabled={disabled}
          onFocus={() => setFocused(true)}
          onBlur={(event) => {
            if (
              !event.currentTarget.contains(event.relatedTarget as Node | null)
            )
              setFocused(false);
          }}
        >
          {value.map((dep) => {
            const { scope, name } = splitKey(dep.tool);
            return (
              <Chip key={dep.tool} invalid={!isVersion(dep.version)}>
                {scope ? <ChipScope>{scope}</ChipScope> : null}
                <ChipName>{name}</ChipName>
                <ChipValue
                  value={dep.version}
                  editable={!disabled}
                  editLabel={`Edit ${dep.tool} version, currently ${dep.version}`}
                  confirmLabel={`Confirm ${dep.tool} version`}
                  onCommit={(version) => update(dep.tool, { version })}
                />
                {dep.allow_builds?.length ? (
                  <ChipOption
                    name={ALLOW_BUILDS}
                    value={dep.allow_builds}
                    label={dep.tool}
                    editable={!disabled}
                    onCommit={(text) => setOption(dep.tool, text)}
                  />
                ) : takesAllowBuilds(dep.tool) && !disabled ? (
                  <ChipOptionAdd
                    label={dep.tool}
                    onCommit={(text) => setOption(dep.tool, text)}
                  />
                ) : null}
                <ChipRemove
                  aria-label={`Remove ${dep.tool}`}
                  disabled={disabled}
                  onClick={() =>
                    onChange(value.filter((item) => item.tool !== dep.tool))
                  }
                />
              </Chip>
            );
          })}
          <ChipInputField
            id={id}
            aria-describedby={`${id}-help`}
            placeholder="node, claude, npm:t3..."
            disabled={disabled}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onRemoveLast={() => onChange(value.slice(0, -1))}
            onKeyDown={(event) => {
              if (event.defaultPrevented) return;
              if (event.key === "Enter") {
                event.preventDefault();
                if (isKey(typed)) add(typed);
              }
              if (event.key === "Escape") {
                event.preventDefault();
                setQuery("");
              }
            }}
          />
        </ChipInputBox>
        {focused && typed ? (
          <ChipInputList aria-label="Tool suggestions">
            {matches.map((tool) => (
              <ChipInputOption key={tool} onSelect={() => add(tool)}>
                <span className="mono">{tool}</span>
                {known.has(tool) ? null : (
                  <span className="muted">as typed</span>
                )}
              </ChipInputOption>
            ))}
            {!matches.length ? (
              <ChipInputEmpty>
                {loading
                  ? "Searching mise..."
                  : searchFailed
                    ? "Catalog unavailable. Type a mise key, such as just or npm:t3."
                    : "No match. Try a backend key, such as npm:t3."}
              </ChipInputEmpty>
            ) : null}
          </ChipInputList>
        ) : null}
      </ChipInput>
      <small className="field-hint" id={`${id}-help`}>
        Type a mise key or search for one. Enter and Tab take a suggestion,
        Backspace removes the last chip. Press a version to change it. npm tools
        take <code>allow_builds=name, name</code> on their <code>+</code>.
      </small>
      {unversioned.length ? (
        <ValidationMessage>
          A version is letters, digits, dots and dashes:{" "}
          {unversioned.map((dep) => dep.tool).join(", ")}.
        </ValidationMessage>
      ) : null}
    </div>
  );
}
