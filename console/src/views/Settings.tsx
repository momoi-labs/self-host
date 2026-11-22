import { useEffect, useRef, type ComponentType, type MouseEvent } from "react";
import {
  Badge,
  Navigation,
  NavigationItem,
  NavigationLink,
  NavigationList,
  PageHeader,
  PageHeaderTitle,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@momoi-labs/kiso-react";

import { GitConnectionsSettings } from "../components/GitConnections.js";
import { Icon, type IconName } from "../components/Icon.js";
import {
  settingsHash,
  settingsPath,
  settingsSections,
  useSettingsFailures,
  type SettingId,
  type SettingsSectionId,
} from "../lib/settingsSections.js";
import { ApiKeys } from "./settings/ApiKeys.js";
import { Passkeys } from "./settings/Passkeys.js";
import { BackupStorage, MaintenanceWindow } from "./settings/Backups.js";
import { YourHost } from "./settings/DnsSetup.js";
import { ApplicationDefaults, AuditHistory, HostAccess, NativeBuildLimits } from "./settings/General.js";
import { PublicCertificates } from "./settings/PublicCertificates.js";
import { ScheduledTasks } from "./settings/ScheduledTasks.js";

const cards: Record<SettingId, ComponentType<{ id: string }>> = {
  applications: ApplicationDefaults,
  "native-build-limits": NativeBuildLimits,
  "audit-history": AuditHistory,
  "api-keys": ApiKeys,
  passkeys: Passkeys,
  host: HostAccess,
  "git-connections": GitConnectionsSettings,
  "your-host": YourHost,
  "public-certificates": PublicCertificates,
  "backup-storage": BackupStorage,
  "maintenance-window": MaintenanceWindow,
  "scheduled-tasks": ScheduledTasks,
};

const icons: Record<SettingsSectionId, IconName> = {
  general: "gear",
  access: "key",
  network: "globe",
  backups: "clock",
};

function plainClick(event: MouseEvent) {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

function revealSetting(setting: SettingId) {
  document.getElementById(`setting-${setting}`)?.scrollIntoView({ block: "start" });
}

export function Settings({ id, onOpen }: { id: string; onOpen: (id: string) => void }) {
  const path = settingsPath(id) ?? { section: "general", setting: null };
  const failures = useSettingsFailures();
  const root = useRef<HTMLDivElement>(null);
  const sections = useRef<HTMLDivElement>(null);
  const mounted = useRef(false);

  useEffect(() => {
    const first = !mounted.current;
    mounted.current = true;
    if (!path.setting) {
      if (!first) {
        root.current?.scrollIntoView({ block: "start" });
        sections.current?.scrollTo({ top: 0 });
      }
      return;
    }
    const setting = path.setting;
    const section = document.getElementById(`settings-${path.section}`);
    if (!section) return;
    const observer = new ResizeObserver(() => revealSetting(setting));
    observer.observe(section);
    const stop = () => observer.disconnect();
    const timer = window.setTimeout(stop, 10_000);
    const events = ["wheel", "touchstart", "keydown", "pointerdown"] as const;
    for (const event of events) window.addEventListener(event, stop, { once: true });
    return () => {
      stop();
      window.clearTimeout(timer);
      for (const event of events) window.removeEventListener(event, stop);
    };
  }, [path.section, path.setting]);

  function open(target: string) {
    return (event: MouseEvent) => {
      if (!plainClick(event)) return;
      event.preventDefault();
      const next = settingsPath(target);
      if (next?.setting && target === id) revealSetting(next.setting);
      else onOpen(target);
    };
  }

  const marker = (section: SettingsSectionId) =>
    failures[section] ? (
      <Badge variant="danger">
        {failures[section]}
        <span className="offscreen"> failing</span>
      </Badge>
    ) : null;

  return (
    <>
      <PageHeader ref={root} className="settings-page">
        <PageHeaderTitle>Settings</PageHeaderTitle>
      </PageHeader>
      <div className="settings-frame">
        <div className="settings-layout">
          <Navigation aria-label="Settings sections" className="settings-nav">
            <NavigationList>
              {settingsSections.map((section) => {
                const target = settingsHash({ section: section.id, setting: null }).slice(10);
                return (
                  <NavigationItem key={section.id}>
                    <NavigationLink href={`/console/#settings/${target}`} active={section.id === path.section} onClick={open(target)}>
                      <Icon name={icons[section.id]} />
                      <span className="grow truncate">{section.title}</span>
                      {marker(section.id)}
                    </NavigationLink>
                    {section.id === path.section ? (
                      <NavigationList className="settings-nav-settings">
                        {section.settings.map((setting) => {
                          const link = settingsHash({ section: section.id, setting: setting.id }).slice(10);
                          return (
                            <NavigationItem key={setting.id}>
                              <NavigationLink href={`/console/#settings/${link}`} onClick={open(link)}>
                                <span className="grow truncate">{setting.title}</span>
                              </NavigationLink>
                            </NavigationItem>
                          );
                        })}
                      </NavigationList>
                    ) : null}
                  </NavigationItem>
                );
              })}
            </NavigationList>
          </Navigation>
          <div className="settings-select">
            <Select value={path.section} onValueChange={(section) => onOpen(section)}>
              <SelectTrigger aria-label="Settings section">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {settingsSections.map((section) => (
                  <SelectItem key={section.id} value={section.id}>
                    <span className="settings-select-item">
                      {section.title}
                      {marker(section.id)}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div ref={sections} className="settings-sections">
            {settingsSections.map((section) => (
              <section key={section.id} id={`settings-${section.id}`} className="settings-section" aria-label={section.title} hidden={section.id !== path.section}>
                {section.settings.map((setting) => {
                  const Card = cards[setting.id];
                  return <Card key={setting.id} id={setting.id} />;
                })}
              </section>
            ))}
          </div>
        </div>
      </div>
    </>
  );
}
