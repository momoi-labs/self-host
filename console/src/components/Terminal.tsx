import { useEffect, useRef, useState } from "react";
import { Button, FormField, EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateIcon, EmptyStateTitle, TerminalIcon } from "@momoi-labs/kiso-react";
import { Terminal as Xterm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, apiKey, failureOf } from "../lib/api.js";
import type { App, ServiceState } from "../lib/types.js";
import { Icon } from "./Icon.js";
import { useToast } from "./Toasts.js";

type Connection = { socket: WebSocket; terminal: Xterm; dispose: () => void };

export function Terminal({ id }: { id: string }) {
  const notify = useToast();
  const host = useRef<HTMLDivElement>(null);
  const connection = useRef<Connection | null>(null);
  const request = useRef<AbortController | null>(null);
  const [services, setServices] = useState<ServiceState[]>([]);
  const [container, setContainer] = useState("");
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState<"idle" | "connecting" | "open" | "closed">("idle");
  const active = status === "connecting" || status === "open";

  async function refresh() {
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    setLoading(true);
    try {
      const response = await api(`/apps/id/${encodeURIComponent(id)}`, { signal: controller.signal });
      if (!response.ok) throw await failureOf(response);
      const app = await response.json() as App;
      const current = app.services ?? [];
      setServices(current);
      setContainer((selected) => current.some((service) => service.container === selected && service.state === "running")
        ? selected : current.find((service) => service.state === "running")?.container ?? "");
    } catch (error) {
      if (!controller.signal.aborted) notify("danger", "Could not load containers", error);
    } finally {
      if (!controller.signal.aborted) setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
    return () => { request.current?.abort(); connection.current?.dispose(); connection.current = null; };
    // The Application detail is keyed by its stable identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  function start() {
    if (!host.current || !container || active) return;
    connection.current?.dispose();
    const terminal = new Xterm({
      cursorBlink: true, fontSize: 14, scrollback: 5000,
      fontFamily: 'ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace',
      theme: { background: "#121116", foreground: "#dedbd7", cursor: "#dedbd7" },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    fit.fit();
    const url = new URL(`/apps/id/${encodeURIComponent(id)}/terminal`, window.location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    let ended = false;
    let disposed = false;
    const send = (value: unknown) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(value));
    };
    const input = terminal.onData((data) => {
      // Keep paste frames below the server's input limit, including UTF-8 text.
      for (let i = 0; i < data.length;) {
        let end = Math.min(i + 4096, data.length);
        if (end < data.length && /[\uD800-\uDBFF]/.test(data[end - 1])) end--;
        send({ type: "input", data: data.slice(i, end) });
        i = end;
      }
    });
    const resize = terminal.onResize(({ cols, rows }) => send({ type: "resize", cols, rows }));
    const observer = new ResizeObserver(() => {
      if (host.current && host.current.clientWidth > 0 && host.current.clientHeight > 0) fit.fit();
    });
    observer.observe(host.current);
    setStatus("connecting");
    socket.onopen = () => {
      if (disposed) { socket.close(); return; }
      send({ key: apiKey(), container, cols: terminal.cols, rows: terminal.rows });
    };
    socket.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) { terminal.write(new Uint8Array(event.data)); return; }
      const message = JSON.parse(event.data);
      if (message.type === "ready") { setStatus("open"); terminal.focus(); }
      if (message.type === "exit") {
        ended = true;
        terminal.write(`\r\n[Session ended: ${message.code}]\r\n`);
      }
      if (message.type === "error") {
        ended = true;
        notify("danger", "Terminal failed", message.report);
      }
    };
    socket.onclose = () => {
      if (disposed) return;
      setStatus("closed");
      if (!ended) notify("danger", "Terminal disconnected", "Start a new terminal to reconnect.");
      terminal.options.disableStdin = true;
    };
    connection.current = { socket, terminal, dispose: () => {
      disposed = true;
      socket.onclose = null;
      socket.onmessage = null;
      socket.close();
      observer.disconnect(); input.dispose(); resize.dispose(); terminal.dispose();
    } };
  }

  function close() {
    connection.current?.dispose();
    connection.current = null;
    setStatus("closed");
  }

  return (
    <div className="container-terminal">
      <div className="terminal-screen" ref={host} aria-label="Container terminal" style={{ visibility: active ? "visible" : "hidden" }} />
      {!active && (
        <EmptyState variant="first-run" className="hatch terminal-empty">
          <EmptyStateIcon><TerminalIcon /></EmptyStateIcon>
          <EmptyStateTitle>Container terminal</EmptyStateTitle>
          <EmptyStateDescription>
            {loading ? "Loading containers..." : container ? "Start an interactive Bash session." : "No running containers available."}
          </EmptyStateDescription>
          {services.length > 1 && (
            <FormField id="terminal-container" label="Container" className="terminal-picker">
              <select id="terminal-container" className="input" value={container} onChange={(event) => setContainer(event.target.value)} disabled={loading}>
                {!container && <option value="">No running containers</option>}
                {services.map((service) => <option key={service.container} value={service.container} disabled={service.state !== "running"}>
                  {service.service}{service.state !== "running" ? ` (${service.state})` : ""}
                </option>)}
              </select>
            </FormField>
          )}
          <EmptyStateActions>
            <Button variant="primary" onClick={start} disabled={loading || !container}>Start terminal</Button>
            {!loading && !container && <Button onClick={() => void refresh()}>Refresh containers</Button>}
          </EmptyStateActions>
        </EmptyState>
      )}
      {active && (
        <div className="terminal-session-controls">
          {status === "connecting" && <span role="status">Connecting...</span>}
          <Button variant="ghost" onClick={close} aria-label="Close terminal" title="Close terminal">
            <Icon name="x" />
          </Button>
        </div>
      )}
    </div>
  );
}
