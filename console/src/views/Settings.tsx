import {
  Card,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@momoi-labs/kiso-react";

import { ApiKeys } from "./settings/ApiKeys.js";
import { DnsSetup } from "./settings/DnsSetup.js";
import { General } from "./settings/General.js";

export type SettingsTab = "general" | "api-keys" | "dns-setup";

export const settingsTabs: readonly SettingsTab[] = ["general", "api-keys", "dns-setup"];

export function isSettingsTab(value: string): value is SettingsTab {
  return (settingsTabs as readonly string[]).includes(value);
}

/**
 * Everything the Operator configures about the Platform itself, one tab per
 * group. The active tab is in the URL fragment, so a link lands on it and a
 * reload keeps it.
 */
export function Settings({ tab, onTab }: { tab: SettingsTab; onTab: (tab: SettingsTab) => void }) {
  return (
    <div className="stack">
      <PageHeader>
        <PageHeaderTitle>Settings</PageHeaderTitle>
        <PageHeaderDescription>How the Platform runs, who can reach it, and how devices find it.</PageHeaderDescription>
      </PageHeader>
      <Card className="detail-tabs">
        <Tabs value={tab} onValueChange={(next) => { if (isSettingsTab(next)) onTab(next); }}>
          <TabsList aria-label="Settings">
            <TabsTrigger value="general">General</TabsTrigger>
            <TabsTrigger value="api-keys">API keys</TabsTrigger>
            <TabsTrigger value="dns-setup">DNS setup</TabsTrigger>
          </TabsList>
          <TabsContent value="general"><General /></TabsContent>
          <TabsContent value="api-keys"><ApiKeys /></TabsContent>
          <TabsContent value="dns-setup"><DnsSetup /></TabsContent>
        </Tabs>
      </Card>
    </div>
  );
}
