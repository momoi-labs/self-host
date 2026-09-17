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
  /** Whether the edit dialog offers a new name. `admin` keeps its own, so
   *  the row stays where the Operator looks for it. */
  renamable: boolean;
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
      // `admin` is an ordinary Record `init` created; the Operator edits it
      // by hand when the Host's address moves (ADR-0025). The wildcard is
      // rebuilt from the interfaces and has nothing to edit.
      const admin = record.owner === "platform" && record.name === "admin";
      rows.set(key, {
        ...record,
        values: [record.value],
        // The Owner column says what kind of thing holds the Record, so the
        // origin is the thing itself: the Application's or machine's name.
        origin: record.owner === "application"
          ? application?.name ?? record.application_id ?? "Unavailable"
          : record.owner === "virtual-machine"
            ? machine?.config.name ?? record.virtual_machine_id ?? "Unavailable"
            : record.name === "*" ? "Wildcard" : admin ? "admin" : "Operator",
        originHref: record.owner === "application" && record.application_id
          ? `/console/#app-${record.application_id}`
          : record.owner === "virtual-machine" && record.virtual_machine_id
            ? `/console/#environment-${record.virtual_machine_id}`
            : null,
        editable: record.owner === "operator" || admin,
        renamable: record.owner === "operator",
      });
    }
  }
  return [...rows.values()];
}
