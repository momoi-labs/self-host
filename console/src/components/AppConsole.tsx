import { Suspense, lazy, useState } from "react";
import { Skeleton, Tabs, TabsContent, TabsList, TabsTrigger } from "@momoi-labs/kiso-react";
import { AppLogPane } from "./LogPane.js";

// xterm is a third of the console's JavaScript and only the Terminal tab needs
// it, so the chunk arrives when the Operator opens that tab.
const Terminal = lazy(() => import("./Terminal.js").then((module) => ({ default: module.Terminal })));

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
        {openedTerminal ? (
          <Suspense fallback={<Skeleton className="terminal-loading" />}>
            <Terminal id={id} />
          </Suspense>
        ) : null}
      </TabsContent>
    </Tabs>
  );
}
