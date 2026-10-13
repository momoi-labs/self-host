import { useState, type ReactNode } from "react";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle, Badge, Button, Dot, DropdownMenu, DropdownMenuContent, DropdownMenuItem,
  DropdownMenuSeparator, DropdownMenuTrigger, Skeleton, Table, TableBody, TableCell, TableFrame, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { kindIcon, kindLabel, redirectFrom, redirectTo, replaceRoute, saveRedirects, saveRoutes, targetLabel, urlOf, type Route } from "../lib/routes.js";
import type { App, RedirectRule } from "../lib/types.js";

/** A redirect as a row: the Application it answers for, the rule, and its place in that Application's list. */
export type RedirectEntry = { app: App; rule: RedirectRule; index: number };
import { statusTone } from "../lib/status.js";
import { HttpStatus } from "./HttpStatus.js";
import { Icon } from "./Icon.js";
import { useToast } from "./Toasts.js";

/**
 * Routes as a list: the Routes page shows every Application's, an
 * Application's Routes tab only its own. The Hostname is renamed in
 * Configuration; aliases and paths are changed here.
 */
export function RouteTable({ routes, redirects = [], total, ready = true, showApplication, toolbar, empty, reload, onEdit, onEditRedirect, onOpenApp, onRename }: {
  /** The routes left after filtering. */
  routes: Route[];
  /** The redirects left after filtering, each with its place in the Application's list. */
  redirects?: RedirectEntry[];
  /** How many there are before filtering, redirects included. */
  total: number;
  ready?: boolean;
  showApplication?: boolean;
  /** The pane's header, inside the frame above the rows. */
  toolbar?: ReactNode;
  /** Said in place of the rows when there are no routes at all. */
  empty: ReactNode;
  /** Reads the Applications again once a route is removed. */
  reload: () => Promise<unknown>;
  onEdit: (route: Route) => void;
  onEditRedirect?: (entry: RedirectEntry) => void;
  onOpenApp?: (id: string) => void;
  /** Where the Hostname is changed, when this screen can go there. */
  onRename?: () => void;
}) {
  const notify = useToast();
  const [removing, setRemoving] = useState<Route | null>(null);
  const [unredirecting, setUnredirecting] = useState<RedirectEntry | null>(null);
  const columns = showApplication ? 6 : 5;

  async function remove(route: Route) {
    try {
      const refused = await saveRoutes(route.app, replaceRoute(route.app, route, null));
      if (refused) { notify("danger", "Could not remove the route", refused); return; }
      await reload();
      notify("success", "Route removed");
    } catch (cause) {
      notify("danger", "Could not remove the route", (cause as Error).message);
    }
  }
  async function removeRedirect({ app, index }: RedirectEntry) {
    try {
      const refused = await saveRedirects(app, (app.redirects ?? []).filter((_, at) => at !== index));
      if (refused) { notify("danger", "Could not remove the route", refused); return; }
      await reload();
      notify("success", "Route removed");
    } catch (cause) {
      notify("danger", "Could not remove the route", (cause as Error).message);
    }
  }
  const appLink = (app: App) => (
    <a className="fg route-app" href={`/console/#app-${app.id}`} onClick={(event) => {
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      onOpenApp?.(app.id);
    }}>
      <Dot variant={statusTone(app.status)} />{app.name}
    </a>
  );
  return (
    <TableFrame aria-busy={!ready}>
      {toolbar}
      <div className="table-scroll">
        <Table aria-label="Routes">
          <TableHeader>
            <TableRow>
              <TableHead scope="col">Route</TableHead>
              {showApplication ? <TableHead scope="col">Application</TableHead> : null}
              <TableHead scope="col">Type</TableHead>
              <TableHead scope="col">Target</TableHead>
              <TableHead scope="col">HTTP</TableHead>
              <TableHead scope="col" className="col-tight">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {!ready ? [0, 1, 2].map((row) => (
              <TableRow key={row}>{Array.from({ length: columns }, (_, cell) => <TableCell key={cell}><Skeleton /></TableCell>)}</TableRow>
            )) : total === 0 ? (
              <TableRow><TableCell colSpan={columns}>{empty}</TableCell></TableRow>
            ) : routes.length === 0 && redirects.length === 0 ? (
              <TableRow><TableCell colSpan={columns}>No routes match your filters.</TableCell></TableRow>
            ) : routes.map((route) => (
              <TableRow key={route.key}>
                <TableCell>
                  <span className="mono">{route.hostname}<span className="route-path">{route.path}</span></span>
                </TableCell>
                {showApplication ? (
                  <TableCell>{appLink(route.app)}</TableCell>
                ) : null}
                <TableCell><Badge variant="neutral"><Icon name={kindIcon[route.kind]} />{kindLabel[route.kind]}</Badge></TableCell>
                <TableCell className="mono muted">{route.target ?? targetLabel(route.app)}{route.path !== "/" && !route.stripPrefix ? route.path : ""}</TableCell>
                <TableCell>
                  {/* The probe reads the Web Target; a rule that names its own target is not checked. */}
                  {route.target ? <span className="muted">Not checked</span> : <HttpStatus id={route.app.id} status={route.app.status} />}
                </TableCell>
                <TableCell className="col-tight">
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for ${urlOf(route)}`}><Icon name="more" /></Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      {showApplication ? <DropdownMenuItem onSelect={() => onOpenApp?.(route.app.id)}>Open {route.app.name}</DropdownMenuItem> : null}
                      <DropdownMenuItem onSelect={() => void navigator.clipboard?.writeText(urlOf(route))}>Copy URL</DropdownMenuItem>
                      {route.kind === "hostname" ? (
                        onRename ? <DropdownMenuItem onSelect={onRename}>Change in Configuration</DropdownMenuItem> : null
                      ) : (
                        <>
                          <DropdownMenuItem onSelect={() => onEdit(route)}>Edit route</DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem className="is-danger" onSelect={() => setRemoving(route)}>Remove route</DropdownMenuItem>
                        </>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                </TableCell>
              </TableRow>
            ))}
            {ready ? redirects.map((entry) => { const { app, rule, index } = entry; return (
              <TableRow key={`redirect:${app.id}:${index}`}>
                <TableCell><span className="mono">{redirectFrom(rule)}</span></TableCell>
                {showApplication ? <TableCell>{appLink(app)}</TableCell> : null}
                <TableCell><Badge variant="neutral"><Icon name={kindIcon.redirect} />Redirect</Badge></TableCell>
                <TableCell className="mono muted">→ {redirectTo(rule)}</TableCell>
                <TableCell><Badge variant="neutral">{rule.permanent ? "301 Permanent" : "302 Temporary"}</Badge></TableCell>
                <TableCell className="col-tight">
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button size="sm" variant="ghost" className="btn-icon" aria-label={`Actions for redirect ${redirectFrom(rule)}`}><Icon name="more" /></Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      {showApplication ? <DropdownMenuItem onSelect={() => onOpenApp?.(app.id)}>Open {app.name}</DropdownMenuItem> : null}
                      <DropdownMenuItem onSelect={() => onEditRedirect?.(entry)}>Edit route</DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem className="is-danger" onSelect={() => setUnredirecting(entry)}>Remove route</DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </TableCell>
              </TableRow>
            ); }) : null}
          </TableBody>
        </Table>
      </div>
      <p className="table-footer" role="status"><span>{ready ? `${routes.length + redirects.length} of ${total} routes` : "Loading routes..."}</span></p>
      <AlertDialog open={removing !== null} onOpenChange={(open) => { if (!open) setRemoving(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader><AlertDialogTitle>Remove route</AlertDialogTitle></AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              <code>{removing ? urlOf(removing) : null}</code> stops reaching {removing?.app.name}.
              {removing?.kind === "path" ? " Its requests go to the hostname's / route, if it has one." : " The proxy answers 404 for it."}
            </AlertDialogDescription>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="btn-danger" onClick={() => { if (removing) void remove(removing); setRemoving(null); }}>Remove route</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={unredirecting !== null} onOpenChange={(open) => { if (!open) setUnredirecting(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader><AlertDialogTitle>Remove route</AlertDialogTitle></AlertDialogHeader>
          <div className="dialog-body">
            <AlertDialogDescription>
              <code>{unredirecting ? redirectFrom(unredirecting.rule) : null}</code> stops redirecting. Requests it matched reach {unredirecting?.app.name} instead.
            </AlertDialogDescription>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="btn-danger" onClick={() => { if (unredirecting) void removeRedirect(unredirecting); setUnredirecting(null); }}>Remove route</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </TableFrame>
  );
}
