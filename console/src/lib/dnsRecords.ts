import type { App, Environment } from "./types.js";

export type DnsRecord = {
  name: string;
  type: "A";
  value: string;
  ttl: number;
  description?: string | null;
  owner: "platform" | "application" | "virtual-machine" | "operator";
  application_id?: string;
  virtual_machine_id?: string;
};

export type RecordRow = DnsRecord & {
  values: string[];
  origin: string;
  originHref: string | null;
  editable: boolean;
};

export function recordRows(
  records: DnsRecord[],
  applications: Pick<App, "id" | "name">[],
  machines: { id: Environment["id"]; config: Pick<Environment["config"], "name"> }[] = [],
): RecordRow[] {
  const rows = new Map<string, RecordRow>();
  for (const record of records) {
    const key = JSON.stringify([record.name, record.type]);
    const existing = rows.get(key);
    if (existing) {
      if (!existing.values.includes(record.value)) existing.values.push(record.value);
    } else {
      const application = applications.find(app => app.id === record.application_id);
      const machine = machines.find(vm => vm.id === record.virtual_machine_id);
      rows.set(key, {
        ...record,
        values: [record.value],
        origin: record.owner === "application"
          ? `App/${application?.name ?? record.application_id ?? "Unavailable"}`
          : record.owner === "virtual-machine"
            ? `VM/${machine?.config.name ?? record.virtual_machine_id ?? "Unavailable"}`
            : record.name === "*" ? "Wildcard" : record.name === "admin" ? "admin" : "Operator",
        originHref: record.owner === "application" && record.application_id
          ? `/console/#app-${record.application_id}`
          : record.owner === "virtual-machine" && record.virtual_machine_id
            ? `/console/#environment-${record.virtual_machine_id}`
            : null,
        editable: record.owner === "operator",
      });
    }
  }
  return [...rows.values()];
}
