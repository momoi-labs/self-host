import {
  Card,
  PageHeader,
  PageHeaderDescription,
  PageHeaderTitle,
} from "@momoi-labs/kiso-react";

import { AppForm, type Submission } from "../components/AppForm.js";
import { useToast } from "../components/Toasts.js";
import { api, failureOf } from "../lib/api.js";
import type { App, Report } from "../lib/types.js";

/**
 * Creating an Application is the edit form on a page of its own, without the
 * log pane: there is nothing to stream yet.
 */
export function NewApp({
  dnsSuffix,
  reload,
  onCancel,
  onCreated,
}: {
  dnsSuffix: string;
  reload: () => Promise<App[]>;
  onCancel: () => void;
  onCreated: (app: App | null) => void;
}) {
  const notify = useToast();

  async function deploy(body: Submission, source: string): Promise<Report | null> {
    try {
      const res = await api("/apps", { method: "POST", body: JSON.stringify(body) });
      const failure = res.ok ? null : await failureOf(res);

      const next = await reload();
      const created = next.find((app) => app.name === body.name) ?? null;

      // A refused name or file never reached the database. Keep the form with
      // what was typed, so the fix is one edit rather than a retype.
      if (failure && !created) return failure;

      // The Application is on record even when the deploy failed, so open it
      // instead of reporting an error the operator cannot act on.
      onCreated(created);

      if (!failure) {
        // Accepted, not finished: the pull runs on the platform and the poll
        // says how it went.
        notify("success", `Deploy started for ${body.name}`, {
          error:
            source === "compose"
              ? "Bringing the Compose project up ..."
              : source === "custom-image"
                ? "Starting the development server ..."
              : source === "native"
                ? "Starting the process under its dedicated Application Account ..."
              : source === "git"
                ? "Checking out the selected commit and building its image ..."
              : `Pulling image ${body.image} ...`,
          caused_by: [],
        });
      }
      // A failure needs no toast: the detail already renders the reason as the
      // application's alert, and that one stays on screen.
      return null;
    } catch (cause) {
      return { error: "Could not reach the platform", caused_by: [(cause as Error).message] };
    }
  }

  return (
    <>
      <PageHeader>
        <PageHeaderTitle>New application</PageHeaderTitle>
        <PageHeaderDescription>
          Deploy a container or native process.
        </PageHeaderDescription>
      </PageHeader>
      <Card className="form-page">
        <AppForm dnsSuffix={dnsSuffix} onSubmit={deploy} onCancel={onCancel} />
      </Card>
    </>
  );
}
