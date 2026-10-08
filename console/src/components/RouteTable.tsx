import { useState, type ReactNode } from "react";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle, Badge, Button, Dot, DropdownMenu, DropdownMenuContent, DropdownMenuItem,
  DropdownMenuSeparator, DropdownMenuTrigger, Skeleton, Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@momoi-labs/kiso-react";

import { kindLabel, replaceRoute, saveRoutes, targetLabel, urlOf, type Route } from "../lib/routes.js";
import { statusTone } from "../lib/status.js";
import { HttpStatus } from "./HttpStatus.js";
import { Icon } from "./Icon.js";
import { useToast } from "./Toasts.js";

/**
 * Routes as a list: the Routes page shows every Application's, an
 * Application's Routes tab only its own. The Hostname is renamed in
 * Configuration; aliases and paths are changed here.
 */
export function RouteTable({ routes, total, ready = true, showApplication, empty, reload, onEdit, onOpenApp, onRename }: {
  /** The routes left after filtering. */
  routes: Route[];
  /** How many there are before filtering. */
  total: number;
  ready?: boolean;
  showApplication?: boolean;
  /** Said in place of the rows when there are no routes at all. */
  empty: ReactNode;
  /** Reads the Applications again once a route is removed. */
  reload: () => Promise<unknown>;
  onEdit: (route: Route) => void;
  onOpenApp?: (id: string) => void;
  /** Where the Hostname is changed, when this screen can go there. */
  onRename?: () => void;
}) {
  const notify = useToast();
  const [removing, setRemoving] = useState<Route | null>(null);
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
  return (
    <div className="table-wrap" aria-busy={!ready}>
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
            ) : routes.length === 0 ? (
              <TableRow><TableCell colSpan={columns}>No routes match your filters.</TableCell></TableRow>
            ) : routes.map((route) => (
              <TableRow key={route.key}>
                <TableCell>
                  <span className="mono">{route.hostname}<span className="route-path">{route.path}</span></span>
                </TableCell>
                {showApplication ? (
                  <TableCell>
                    <a className="fg route-app" href={`/console/#app-${route.app.id}`} onClick={(event) => {
                      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                      event.preventDefault();
                      onOpenApp?.(route.app.id);
                    }}>
                      <Dot variant={statusTone(route.app.status)} />{route.app.name}
                    </a>
                  </TableCell>
                ) : null}
                <TableCell><Badge variant="neutral">{kindLabel[route.kind]}</Badge></TableCell>
                <TableCell className="mono muted">{route.target ?? targetLabel(route.app)}</TableCell>
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
          </TableBody>
        </Table>
      </div>
      <p className="table-footer" role="status"><span>{ready ? `${routes.length} of ${total} routes` : "Loading routes..."}</span></p>
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
    </div>
  );
}
