import { useState } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@momoi-labs/kiso-react";
import { AppLogPane } from "./LogPane.js";
import { Terminal } from "./Terminal.js";

export function AppConsole({ id }: { id: string }) {
  const [tab, setTab] = useState("logs");
  const [openedTerminal, setOpenedTerminal] = useState(false);
  return (
    <Tabs className="app-console" value={tab} onValueChange={(tab) => {
      setTab(tab);
      if (tab === "terminal") setOpenedTerminal(true);
    }}>
      <TabsList aria-label="Application output">
        <TabsTrigger value="logs">Logs</TabsTrigger>
        <TabsTrigger value="terminal">Terminal</TabsTrigger>
      </TabsList>
      <TabsContent value="logs" className="app-console-logs"><AppLogPane id={id} /></TabsContent>
      <TabsContent value="terminal" forceMount hidden={tab !== "terminal"}>
        {openedTerminal ? <Terminal id={id} /> : null}
      </TabsContent>
    </Tabs>
  );
}
