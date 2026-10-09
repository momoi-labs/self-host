# Application route rules

An Application can route one Hostname to several loopback targets by path.
The proxy reads route changes on the next request, without restarting an
Application. The Application record owns its rules, so deleting or stopping
one Application removes only its routes.

## Configure rules

`POST /apps` and `PUT /apps/id/{id}` accept `route_rules`. Responses include
the saved list. An omitted field preserves the current rules on update;
`[]` removes the explicit rules. Records created before this field existed
read an empty list.

`PUT /apps/id/{id}/routes` changes only `aliases`, `route_rules` and
`rewrite_host` (see [Host header](#host-header)), with
the same rules for an omitted field and `[]`. It never builds, pulls or
restarts, whatever the Application is made from, so a Git Application can
gain a path without a new build. It answers `200` once the proxy serves the
new routes.

```json
{
  "route_rules": [
    {
      "hostname": "blog.example.invalid",
      "path_prefix": "/app",
      "target": "127.0.0.1:20002",
      "strip_prefix": true
    }
  ]
}
```

The Application's Hostname and aliases keep their automatic `/` routes to
its Web Target. An explicit `/` rule replaces the same Application's
automatic root route for that Hostname. A rule can also use another
Application's Hostname when its path key is free. A rule without a
`target` follows its Application's Web Target, so a redeploy that moves the
Host port keeps it working. A set target must be an IPv4 or IPv6 loopback
address with a nonzero port.

An unpublished Application cannot have rules. Native Applications use the
same route contract. The route contract itself
uses socket addresses, so the proxy does not depend on how a target runs.

## Host header

The proxy forwards the `Host` the client sent, so an Application sees its
Hostname. Some servers refuse any `Host` that is not a loopback name, as a
guard against DNS rebinding. For those, the proxy can send the target's
address, such as `127.0.0.1:8642`, as `Host` on every route of the
Application, including its Hostname and aliases. `X-Forwarded-Host` still
carries the Hostname.

The Operator sets the default with `rewriteHost` on `PUT /settings`. It is off
until turned on, and applies to the next request. An Application overrides it
with `rewrite_host` on `PUT /apps/id/{id}/routes`: `true` or `false` pins its
choice, and `null` follows the setting again. An omitted field keeps the
current choice, and a redeploy keeps it too.

## Matching and validation

The longest matching prefix wins. A prefix ends at a path segment boundary:
`/app` matches `/app`, `/app/` and `/app/assets/main.css`, but not `/apple`.
Matching is case-sensitive for paths. Hostnames are stored in lowercase;
requests match Hostnames regardless of case.

Prefixes must start with `/`. Except for `/` itself, they cannot end with a
slash or contain empty segments, `.` or `..` segments, percent escapes,
queries, fragments, backslashes or non-ASCII characters. The query stays
unchanged when the proxy forwards a request.

A duplicate explicit `(hostname, path_prefix)` key is a conflict, even if
both rules name the same target. A key already owned by another Application
is also a conflict. The API returns `409` with the existing
`{error, caused_by}` response shape. Invalid rules return `400`. Neither
case replaces the saved rules.

The management Hostname is reserved for the Platform. The proxy handles
`/.well-known/acme-challenge` and its descendants before Application rules,
management routes and HTTP redirects. An explicit rule for that subtree is
refused. A broader rule such as `/` or `/.well-known` never captures a
challenge request. The reservation also covers percent-encoded spellings
and normalized dot segments that an upstream might otherwise decode.

## Prefix behavior and upstream compatibility

With `strip_prefix: false`, the upstream receives the original path. With
`true`, `/app/assets/main.css?version=2` reaches the target as
`/assets/main.css?version=2`, and `/app` becomes `/`. The proxy sets
`X-Forwarded-Prefix: /app` for a stripped prefix. It removes a caller's
supplied value first.

The proxy forwards response bodies, redirect locations, cookies and asset
links unchanged. Configure the upstream's base URL or prefix support so it
generates Consumer URLs under `/app`. An upstream that always redirects to
`/login` or loads assets from `/assets` cannot run under `/app` without its
own prefix configuration. The fixture suite records this limitation instead
of rewriting arbitrary HTML or redirect responses.

Request and response bodies stream through the proxy. WebSocket handshakes
keep authentication cookies, Origin and subprotocol negotiation, then
forward frames over the upgraded connection.

## Certificate challenge boundary

W2 can install its HTTP-01 handler with
`proxy::Bound::with_challenge_router(axum::Router)` before `run`. The router
receives the original request, including its Hostname and full challenge
path, on both HTTP and HTTPS. The default router returns `404` for every
challenge. Missing challenges remain `404`; they cannot fall through
to an Application or an HTTP-to-HTTPS redirect. W2 installs the certificate manager on this boundary. See
[public certificates](public-certificates.md) for issuance and renewal.

## Evidence

The routing tests use synthetic Hostnames and disposable loopback listeners.
They cover longest-prefix and segment matching, preserved queries, assets,
prefix-aware redirects, the root-only redirect limitation, request and response
streaming and authenticated WebSockets. They also exercise hot updates,
shared-Hostname ownership, conflict rollback and both challenge listeners.
