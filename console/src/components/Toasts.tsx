import { useCallback } from "react";
import { ToastDescription, useToast as useKisoToast } from "@momoi-labs/kiso-react";

import { asReport } from "../lib/api.js";
import { Causes } from "./Causes.js";

type Notify = (kind: "success" | "danger", title: string, body?: unknown) => void;

/** Keeps the Platform's error and cause chain together in a notification. */
export function useToast(): Notify {
  const notify = useKisoToast();
  return useCallback<Notify>((kind, title, body) => {
    const report = body ? asReport(body) : null;
    notify(kind === "danger" ? "error" : "success", title, report ? (
      <>
        <ToastDescription>{report.error}</ToastDescription>
        <Causes report={report} />
      </>
    ) : undefined);
  }, [notify]);
}
