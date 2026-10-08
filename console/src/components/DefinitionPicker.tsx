import { DetailSelect } from "@momoi-labs/kiso-react";

import { isCompose } from "../lib/status.js";
import type { App } from "../lib/types.js";
import { Icon, type IconName } from "./Icon.js";

export type Definition = "image" | "custom-image" | "compose" | "git" | "native";

/** Each definition, when to choose it, and how the Platform runs it. */
const definitions: {
  value: Definition;
  label: string;
  icon: IconName;
  guidance: string;
  flow: [IconName, string][];
}[] = [
  {
    value: "image",
    label: "Container image",
    icon: "box",
    guidance: "An image that is already published, such as nginx:alpine.",
    flow: [["download", "Pulls the image"], ["box", "Runs one container"], ["globe", "Hostname reaches port 80"]],
  },
  {
    value: "custom-image",
    label: "Custom image",
    icon: "hammer",
    guidance: "A development server from an image built on this Host.",
    flow: [["hammer", "Uses a saved image"], ["box", "Runs your start command"], ["globe", "Hostname reaches its web port"]],
  },
  {
    value: "compose",
    label: "Compose file",
    icon: "layers",
    guidance: "Several services that run together, from one Compose file.",
    flow: [["file", "Reads the Compose file"], ["layers", "Runs one container per service"], ["globe", "Hostname reaches the web service"]],
  },
  {
    value: "git",
    label: "Git repository",
    icon: "branch",
    guidance: "Source with a Dockerfile or Compose file, built on this Host.",
    flow: [["branch", "Checks out a commit"], ["hammer", "Builds its images"], ["globe", "Hostname reaches the web port"]],
  },
  {
    value: "native",
    label: "Native process",
    icon: "terminal",
    guidance: "A program that runs on the Host itself, without a container.",
    flow: [["download", "Installs tools with mise"], ["user", "Runs as its own Application Account"], ["globe", "Hostname reaches a loopback port"]],
  },
];

/** The definition an existing Application was made from. */
export function definitionOf(app: App): Definition {
  return app.runtime?.kind === "native" ? "native" : app.git ? "git" : app.development ? "custom-image" : isCompose(app) ? "compose" : "image";
}

/** A definition's label and pictogram, for a row that names what an Application runs. */
export function describeDefinition(value: Definition): { label: string; icon: IconName } {
  const { label, icon } = definitions.find((one) => one.value === value) ?? definitions[0];
  return { label, icon };
}

/**
 * The first choice on "New application": what the Application is made from.
 * Each option shows its pictogram, when to choose it and how the Platform
 * runs it, step by step, so the form below it is never a surprise.
 */
export function DefinitionPicker({
  value,
  onChange,
  disabled,
}: {
  value: Definition;
  onChange: (next: Definition) => void;
  disabled?: boolean;
}) {
  const chosen = definitions.find((one) => one.value === value) ?? definitions[0];
  return (
    <div className="field definition-picker">
      <DetailSelect
        id="f-definition"
        label="Definition"
        value={chosen.value}
        disabled={disabled}
        aria-describedby="f-definition-hint"
        onValueChange={(next) => onChange(next as Definition)}
        options={definitions.map((one) => ({
          value: one.value,
          label: one.label,
          icon: <Icon name={one.icon} />,
          illustration: <span className="definition-illustration"><Icon name={one.icon} size="lg" /></span>,
          description: (
            <>
              <p>{one.guidance}</p>
              <ol className="definition-steps" aria-label={`How a ${one.label.toLowerCase()} runs`}>
                {one.flow.map(([icon, caption]) => (
                  <li key={caption}>
                    <Icon name={icon} />
                    {caption}
                  </li>
                ))}
              </ol>
            </>
          ),
        }))}
      />
      <small id="f-definition-hint" className="field-hint">{chosen.guidance}</small>
    </div>
  );
}
