import { useMemo, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
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

import type { ImageDependency } from "../lib/customImageTemplates.js";
import {
  ALLOW_BUILDS,
  isKey,
  isVersion,
  setOption,
  splitKey,
  suggest,
} from "../lib/dependencies.js";
import { toolCatalogQuery, type MiseTool } from "../lib/queries.js";

const noTools: MiseTool[] = [];

/** Shared catalog-backed mise dependency editor for images and environments. */
export function Dependencies({
  value,
  onChange,
  disabled,
  id = "dependency-search",
  hint,
}: {
  value: ImageDependency[];
  onChange: (value: ImageDependency[]) => void;
  disabled: boolean;
  id?: string;
  hint?: ReactNode;
}) {
  const catalog = useQuery(toolCatalogQuery);
  const tools = catalog.data ?? noTools;
  const loading = catalog.isLoading;
  const searchFailed = catalog.isError;
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
  function commitOption(dep: ImageDependency, text: string, previous?: string) {
    const next = setOption(dep, text, previous);
    if (next) onChange(value.map((item) => (item.tool === dep.tool ? next : item)));
  }

  return (
    <div className="field">
      <Label htmlFor={id}>
        <a href="https://mise.jdx.dev/" target="_blank" rel="noreferrer noopener">
          Mise
        </a>{" "}
        packages and dependencies
      </Label>
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
                    onCommit={(text) => commitOption(dep, text, ALLOW_BUILDS)}
                  />
                ) : null}
                {Object.entries(dep.options ?? {}).map(([name, values]) => (
                  <ChipOption
                    key={name}
                    name={name}
                    value={values.length === 1 ? values[0] : values}
                    label={dep.tool}
                    editable={!disabled}
                    onCommit={(text) => commitOption(dep, text, name)}
                  />
                ))}
                {!disabled ? (
                  <ChipOptionAdd
                    label={dep.tool}
                    onCommit={(text) => commitOption(dep, text)}
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
        {hint ?? <>Type a mise key or search for one. Enter and Tab take a suggestion,
        Backspace removes the last chip. Press a version to change it.</>}{" "}
        Press <code>+</code> on a chip to add a mise option as{" "}
        <code>name=value</code>, such as <code>extras=serve,ane</code> on a pypi
        tool. A list takes its items comma separated.
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
