import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import RFB from "@novnc/novnc";
import {
  Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, EmptyStateActions, EmptyStateDescription, EmptyStateIcon, EmptyStateTitle, FormField,
} from "@momoi-labs/kiso-react";

import { apiKey } from "../lib/api.js";
import { hostQuery } from "../lib/queries.js";
import { Icon } from "./Icon.js";
import { useToast } from "./Toasts.js";

type Status = "idle" | "connecting" | "open";

/**
 * The Mac's own screen, through Screen Sharing, in a window of its own. The
 * Platform carries the bytes; the Mac asks for one of its accounts before it
 * shows anything, and that sign-in needs a secure page, so the console must
 * be open over HTTPS.
 */
export function HostScreen() {
  const notify = useToast();
  const { data: host, isPending, refetch, isFetching } = useQuery(hostQuery);
  const view = useRef<HTMLDivElement>(null);
  const session = useRef<{ rfb: RFB | null; socket: WebSocket } | null>(null);
  const [status, setStatus] = useState<Status>("idle");
  const [asking, setAsking] = useState(false);
  const active = status !== "idle";

  useEffect(() => {
    document.title = "Host screen — self-host";
    return () => end();
  }, []);

  function end() {
    const current = session.current;
    session.current = null;
    if (!current) return;
    current.socket.onmessage = null;
    current.socket.onclose = null;
    if (current.rfb) current.rfb.disconnect();
    else current.socket.close();
  }

  function start() {
    if (!view.current || active) return;
    const url = new URL("/host/screen", window.location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    session.current = { rfb: null, socket };
    setStatus("connecting");
    socket.onopen = () => socket.send(JSON.stringify({ key: apiKey() }));
    socket.onclose = () => {
      session.current = null;
      setStatus("idle");
      notify("danger", "Remote screen disconnected", "Connect again to retry.");
    };
    socket.onmessage = (event) => {
      if (typeof event.data !== "string" || !view.current) return;
      const message = JSON.parse(event.data);
      if (message.type === "error") {
        socket.onclose = null;
        session.current = null;
        setStatus("idle");
        notify("danger", "Remote screen failed", message.report);
        void refetch();
        return;
      }
      if (message.type !== "ready") return;
      // From here the socket carries the VNC protocol. noVNC takes it over
      // before the next frame arrives, so nothing is lost in between.
      socket.onclose = null;
      socket.onmessage = null;
      const rfb = new RFB(view.current, socket);
      rfb.scaleViewport = true;
      rfb.background = "#121116";
      rfb.addEventListener("credentialsrequired", () => setAsking(true));
      rfb.addEventListener("connect", () => { setStatus("open"); rfb.focus(); });
      rfb.addEventListener("securityfailure", (event) => {
        const detail = (event as CustomEvent<{ reason?: string }>).detail;
        notify("danger", "The Mac refused the sign-in", detail.reason || "Check the user name and password.");
      });
      rfb.addEventListener("disconnect", (event) => {
        const clean = (event as CustomEvent<{ clean: boolean }>).detail.clean;
        // A close the Operator asked for has already cleared the session.
        if (session.current?.rfb !== rfb) return;
        session.current = null;
        setAsking(false);
        setStatus("idle");
        if (!clean) notify("danger", "Remote screen disconnected", "Connect again to retry.");
      });
      session.current = { rfb, socket };
    };
  }

  function close() {
    end();
    setAsking(false);
    setStatus("idle");
  }

  function signIn(username: string, password: string) {
    setAsking(false);
    session.current?.rfb?.sendCredentials({ username, password });
  }

  const secure = window.isSecureContext;
  const blocked = !host
    ? (isPending ? "Checking the Host..." : "Could not read the Host's screen status.")
    : !host.screen.supported ? "The remote screen needs a macOS Host."
    : !host.screen.allowed ? "Remote screen is off. Turn it on in Settings to open the Mac's screen here."
    : !host.screen.listening ? "Screen Sharing does not answer on the Mac. Turn it on in System Settings > General > Sharing."
    : !secure ? "Open the console over HTTPS. The Mac's sign-in only works on a secure page."
    : null;

  // The window was opened to connect, so it does, once, as soon as it can.
  const tried = useRef(false);
  useEffect(() => {
    if (blocked !== null || tried.current) return;
    tried.current = true;
    start();
    // start reads the refs it needs when it runs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [blocked]);

  return (
    <div className="remote-screen remote-screen-window">
      <div className="remote-screen-view" ref={view} aria-label="Host screen" style={{ visibility: active ? "visible" : "hidden" }} />
      {!active && (
        <EmptyState variant="first-run" className="hatch remote-screen-empty">
          <EmptyStateIcon><Icon name="screen" /></EmptyStateIcon>
          <EmptyStateTitle>Host screen</EmptyStateTitle>
          <EmptyStateDescription>
            {blocked ?? "Open the Mac's screen. The Mac asks for one of its accounts before it shows anything."}
          </EmptyStateDescription>
          <EmptyStateActions>
            {host && host.screen.supported && !host.screen.allowed ? (
              <Button variant="primary" onClick={() => window.location.assign("/console/#settings/host")}>Open Settings</Button>
            ) : (
              <Button variant="primary" onClick={start} disabled={blocked !== null}>Connect</Button>
            )}
            {host && host.screen.supported && host.screen.allowed && !host.screen.listening ? (
              <Button onClick={() => void refetch()} disabled={isFetching}>Check again</Button>
            ) : null}
          </EmptyStateActions>
        </EmptyState>
      )}
      {active && (
        <div className="remote-screen-controls">
          {status === "connecting" && <span role="status">Connecting...</span>}
          <Button variant="ghost" onClick={close} aria-label="Disconnect" title="Disconnect">
            <Icon name="x" />
          </Button>
        </div>
      )}
      <SignIn open={asking} onCancel={close} onSignIn={signIn} />
    </div>
  );
}

/** The Mac's own sign-in: one of its accounts, not the Platform's API key. */
function SignIn({ open, onCancel, onSignIn }: {
  open: boolean;
  onCancel: () => void;
  onSignIn: (username: string, password: string) => void;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  function submit(event: FormEvent) {
    event.preventDefault();
    onSignIn(username.trim(), password);
    setPassword("");
  }

  return (
    <Dialog open={open} onOpenChange={(next) => { if (!next) onCancel(); }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Sign in to the Mac</DialogTitle>
          <DialogDescription>Screen Sharing asks for an account on the Mac. The console does not keep it.</DialogDescription>
        </DialogHeader>
        <form onSubmit={submit}>
          <DialogBody>
            <FormField id="host-screen-user" label="User name" value={username} autoFocus autoComplete="username"
              onChange={(event) => setUsername(event.target.value)} />
            <FormField id="host-screen-password" label="Password" type="password" value={password} autoComplete="current-password"
              onChange={(event) => setPassword(event.target.value)} />
          </DialogBody>
          <DialogFooter>
            <Button size="sm" type="button" onClick={onCancel}>Cancel</Button>
            <Button size="sm" type="submit" variant="primary" disabled={!username.trim() || !password}>Sign in</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
