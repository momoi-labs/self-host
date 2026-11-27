import { RadioGroup, RadioGroupItem } from "@momoi-labs/kiso-react";

const OPTIONS = [
  ["all", "All"],
  ["running", "Running"],
  ["failed", "Failed"],
  ["stopped", "Stopped"],
] as const;

export function StatusFilter({ label, value, onChange }: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="status-filter">
      <RadioGroup variant="segmented" label={<span className="offscreen">{label}</span>} value={value} onValueChange={onChange}>
        {OPTIONS.map(([option, text]) => <RadioGroupItem key={option} value={option} label={text} />)}
      </RadioGroup>
    </div>
  );
}
