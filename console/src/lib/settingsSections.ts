import { useQuery } from "@tanstack/react-query";

import { useGitConnections } from "./gitConnections.js";
import { apiKeysQuery, certificatesQuery } from "./queries.js";

export const settingsSections = [
  {
    id: "general",
    title: "General",
    settings: [
      { id: "applications", title: "Applications" },
      { id: "native-build-limits", title: "Native Git build limits" },
      { id: "audit-history", title: "Audit history" },
    ],
  },
  {
    id: "access",
    title: "Access",
    settings: [
      { id: "api-keys", title: "API keys" },
      { id: "host", title: "Host" },
      { id: "git-connections", title: "Git connections" },
    ],
  },
  {
    id: "network",
    title: "Network",
    settings: [
      { id: "your-host", title: "Your Host" },
      { id: "public-certificates", title: "Public certificates" },
    ],
  },
  {
    id: "backups",
    title: "Backups and tasks",
    settings: [
      { id: "backup-storage", title: "Backup storage" },
      { id: "maintenance-window", title: "Maintenance window" },
      { id: "scheduled-tasks", title: "Scheduled tasks" },
    ],
  },
] as const;

export type SettingsSectionId = (typeof settingsSections)[number]["id"];
export type SettingId = (typeof settingsSections)[number]["settings"][number]["id"];
export type SettingsPath = { section: SettingsSectionId; setting: SettingId | null };

const moved: Record<string, string> = {
  "api-keys": "access/api-keys",
  "backup-storage": "backups/backup-storage",
  "dns-setup": "network/your-host",
  "git-connections": "access/git-connections",
  host: "access/host",
  "public-certificates": "network/public-certificates",
  "scheduled-tasks": "backups/scheduled-tasks",
};

export function settingsPath(rest: string): SettingsPath | null {
  const [sectionId, settingId, extra] = (moved[rest] ?? rest).split("/");
  if (extra !== undefined) return null;
  const section = settingsSections.find((one) => one.id === sectionId);
  if (!section) return null;
  if (settingId === undefined) return { section: section.id, setting: null };
  const setting = section.settings.find((one) => one.id === settingId);
  return setting ? { section: section.id, setting: setting.id } : null;
}

export function settingsHash(path: SettingsPath): string {
  return path.setting ? `#settings/${path.section}/${path.setting}` : `#settings/${path.section}`;
}

export function useSettingsFailures(): Record<SettingsSectionId, number> {
  const now = Date.now();
  const keys = useQuery(apiKeysQuery).data ?? [];
  const certificates = useQuery(certificatesQuery).data ?? [];
  const { connections } = useGitConnections();
  const expiredKeys = keys.filter((key) => key.expires_at && Date.parse(key.expires_at) <= now).length;
  const expiredConnections = connections.filter((connection) => connection.status === "expired").length;
  const failingCertificates = certificates.filter((certificate) => certificate.state === "renewal_failed" || certificate.state === "expired" || certificate.last_error).length;
  return {
    general: 0,
    access: expiredKeys + expiredConnections,
    network: failingCertificates,
    backups: 0,
  };
}
