import {
  Card,
  CardContent,
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
          From a container image or a Compose file. It will be reachable on {dnsSuffix}.
        </PageHeaderDescription>
      </PageHeader>
      <Card className="form-page">
        <CardContent>
          <AppForm dnsSuffix={dnsSuffix} onSubmit={deploy} onCancel={onCancel} />
        </CardContent>
      </Card>
    </>
  );
}
