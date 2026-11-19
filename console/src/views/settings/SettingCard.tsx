import type { ReactNode } from "react";
import { Card, CardContent, CardHeader } from "@momoi-labs/kiso-react";

export function SettingCard({ id, title, actions, children }: {
  id: string;
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Card id={`setting-${id}`} className="setting-card" aria-labelledby={`setting-${id}-title`} role="region">
      <CardHeader className={actions ? "between" : undefined}>
        <h2 id={`setting-${id}-title`} className="t-h3">{title}</h2>
        {actions ? <div className="setting-card-actions">{actions}</div> : null}
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  );
}
