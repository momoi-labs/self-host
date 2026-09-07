import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import {
  Toast,
  ToastClose,
  ToastContent,
  ToastDescription,
  ToastProvider,
  ToastTitle,
  ToastViewport,
} from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { Causes } from "./Causes.js";
import { Icon } from "./Icon.js";

type Kind = "success" | "danger";

type Notice = {
  id: number;
  kind: Kind;
  title: string;
  body: unknown;
};

type Notify = (kind: Kind, title: string, body?: unknown) => void;

const ToastContext = createContext<Notify>(() => {});

/** Raises a toast from anywhere under <Toasts>. */
export function useToast(): Notify {
  return useContext(ToastContext);
}

let nextId = 0;

/**
 * A toast shows up detached from whatever raised it, so the title has to name
 * the Application. The failure and its causes go in the body, under it.
 */
export function Toasts({ children }: { children: ReactNode }) {
  const [notices, setNotices] = useState<Notice[]>([]);

  const notify = useCallback<Notify>((kind, title, body) => {
    setNotices((current) => [...current, { id: nextId++, kind, title, body }]);
  }, []);

  const dismiss = useCallback((id: number) => {
    setNotices((current) => current.filter((notice) => notice.id !== id));
  }, []);

  const value = useMemo(() => notify, [notify]);

  return (
    <ToastContext.Provider value={value}>
      <ToastProvider duration={6000}>
        {children}
        {notices.map((notice) => {
          const report = notice.body ? asReport(notice.body) : null;
          return (
            <Toast
              key={notice.id}
              variant={notice.kind === "danger" ? "error" : "success"}
              onOpenChange={(open) => {
                if (!open) dismiss(notice.id);
              }}
            >
              <Icon
                name={notice.kind === "danger" ? "alert" : "check"}
                size="md"
                className={notice.kind === "danger" ? "danger" : "success"}
              />
              <ToastContent>
                <ToastTitle>{notice.title}</ToastTitle>
                {report ? (
                  <>
                    <ToastDescription>{report.error}</ToastDescription>
                    <Causes report={report} />
                  </>
                ) : null}
              </ToastContent>
              <ToastClose aria-label="Dismiss">
                <Icon name="x" />
              </ToastClose>
            </Toast>
          );
        })}
        <ToastViewport />
      </ToastProvider>
    </ToastContext.Provider>
  );
}
