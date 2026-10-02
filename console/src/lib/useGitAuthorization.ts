import { useEffect, useRef, useState } from "react";
import {
  completeGitAuthorization,
  cancelGitAuthorization,
  type AuthorizationResult,
  type AuthorizationStart,
  type GitProvider,
} from "./gitConnections.js";
import { authorizationDestination, beginAuthorization, type AuthorizationPhase } from "./gitAuthorization.js";

export function useGitAuthorization(onComplete: (result: AuthorizationResult) => void) {
  const [phase, setPhase] = useState<AuthorizationPhase>("idle");
  const [error, setError] = useState<string | null>(null);
  const active = useRef<ReturnType<typeof beginAuthorization<AuthorizationResult>> | null>(null);
  const mounted = useRef(true);
  const completed = useRef(onComplete);
  completed.current = onComplete;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; void active.current?.cancel(true); };
  }, []);

  function authorize(provider: GitProvider, start: () => Promise<AuthorizationStart>, registration = false) {
    if (active.current) return;
    setError(null);
    const handle = beginAuthorization<AuthorizationResult>({
      origin: window.location.origin,
      target: window,
      openPopup: () => {
        const popup = window.open("about:blank", `self-host-git-${crypto.randomUUID()}`, "popup,width=760,height=800");
        if (popup) {
          popup.document.title = "Connect Git";
          const message = popup.document.createElement("p");
          message.textContent = "Opening the provider's authorization page...";
          popup.document.body.append(message);
        }
        return popup;
      },
      start,
      complete: completeGitAuthorization,
      cancel: cancelGitAuthorization,
      nextSession: (result) => result.authorization ?? null,
      onPhase: (next) => { if (mounted.current) setPhase(next); },
      navigate: (opened, session) => {
        const popup = opened as Window;
        const destination = authorizationDestination(session, provider, registration && session.form !== null);
        if (session.form) {
          const form = popup.document.createElement("form");
          form.method = "post";
          form.action = destination.href;
          const manifest = popup.document.createElement("input");
          manifest.type = "hidden";
          manifest.name = "manifest";
          manifest.value = session.form.manifest;
          form.append(manifest);
          popup.document.body.append(form);
          form.submit();
        } else popup.location.replace(destination.href);
        popup.focus();
      },
    });
    active.current = handle;
    void handle.promise.then((result) => {
      if (mounted.current) completed.current(result);
    }).catch((cause) => {
      if (mounted.current) setError(cause instanceof Error ? cause.message : "Could not authorize Git access. Try again.");
    }).finally(() => {
      if (active.current === handle) active.current = null;
    });
  }

  return { phase, error, busy: phase !== "idle", authorize, cancel: () => active.current?.cancel() ?? Promise.resolve() };
}
