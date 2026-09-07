import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@momoi-labs/kiso-react";

import { serviceTone } from "../lib/status.js";
import type { ServiceState } from "../lib/types.js";
import { StatusBadge } from "./StatusBadge.js";

/**
 * Every container of the Application and what Docker says about it. For a
 * one-container Application this is one row; it is still the row that says
 * "exited with code 1" when the deploy went fine and the process did not.
 */
export function Services({ services }: { services: ServiceState[] }) {
  if (!services.length) return null;

  return (
    <div className="services">
      <p className="t-caps">Services</p>
      <div className="table-wrap">
        <div className="table-scroll">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Service</TableHead>
                <TableHead scope="col">Container</TableHead>
                <TableHead scope="col">State</TableHead>
                <TableHead scope="col" className="num">
                  Restarts
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {services.map((service) => (
                <TableRow key={service.container}>
                  <TableCell className="mono">{service.service}</TableCell>
                  <TableCell className="mono">{service.container}</TableCell>
                  <TableCell>
                    <StatusBadge tone={serviceTone(service.state)}>
                      {service.state}
                      {service.state === "exited" && service.exit_code !== undefined
                        ? ` (${service.exit_code})`
                        : ""}
                    </StatusBadge>
                  </TableCell>
                  <TableCell className="num">
                    {service.restarts === undefined ? (
                      <span className="muted">—</span>
                    ) : (
                      service.restarts
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </div>
    </div>
  );
}
