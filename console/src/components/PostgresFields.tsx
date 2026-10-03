import { FormField, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@momoi-labs/kiso-react";

export function PostgresFields({ major, onChange }: { major: number; onChange: (major: number) => void }) {
  return <Select value={String(major)} onValueChange={(value) => onChange(Number(value))}>
    <FormField id="postgres-major" label="PostgreSQL version" hint="The major version stays fixed."><SelectTrigger><SelectValue /></SelectTrigger></FormField>
    <SelectContent>{[18, 17, 16].map((version) => <SelectItem key={version} value={String(version)}>PostgreSQL {version}</SelectItem>)}</SelectContent>
  </Select>;
}
