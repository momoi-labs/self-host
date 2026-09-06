# Running Hermes

Hermes is the first Application the Compose path was built for. This is the
workflow from an empty Host to a chat in a browser on the LAN. The Compose
subset it relies on is in [compose-applications.md](compose-applications.md);
the Host it assumes is in [macos-host.md](macos-host.md).

Facts about the image below were observed on September 5, 2026 with
`nousresearch/hermes-agent:latest` on Linux/amd64. The image is published for
`linux/arm64` as well.

## What the image does

- `gateway run` starts without a prior `setup`. The image writes its
  configuration, sessions and credentials under `/opt/data` on first start.
- With `HERMES_DASHBOARD=1` the web dashboard listens on port 9119. Bound to
  a non-loopback address, as it is in a container, it requires
  authentication: `HERMES_DASHBOARD_BASIC_AUTH_USERNAME` and
  `HERMES_DASHBOARD_BASIC_AUTH_PASSWORD` are the simplest provider.
- A provider key in the environment, such as `OPENROUTER_API_KEY`, is read
  into Hermes's credential pool at startup. The interactive `setup` wizard
  is not required for the browser workflow.
- The container runs as its own `hermes` user and takes ownership of
  `/opt/data`. Port 8642 is the gateway API for messaging clients and the
  desktop app; the browser does not use it.

## Deploy through the console

1. Open `https://admin.<suffix>` and choose **Deploy application**.
2. Name: `hermes`. Definition: **Compose file**. Paste:

   ```yaml
   services:
     hermes:
       image: nousresearch/hermes-agent:latest
       restart: unless-stopped
       command: gateway run
       volumes:
         - ~/.hermes:/opt/data
       environment:
         - HERMES_DASHBOARD=1
         - HERMES_DASHBOARD_BASIC_AUTH_USERNAME=<username>
         - HERMES_DASHBOARD_BASIC_AUTH_PASSWORD=<password>
         - OPENROUTER_API_KEY=<key>
       deploy:
         resources:
           limits:
             memory: 4G
             cpus: "2.0"
   ```

   Use the provider you have. Hermes reads the usual variable names:
   `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`. Add
   `ports: ["8642:8642"]` only if a messaging client or the desktop app will
   connect from the LAN.
3. Web service: `hermes`. Web port: `9119`. Deploy.
4. The first deploy pulls the image, which takes a few minutes. The
   Application shows `pending`, then `running`. The Services table on the
   detail page shows the container state and the logs pane shows the
   gateway starting; the line `HERMES_DASHBOARD_READY port=9119` is the
   dashboard being up.

Same thing from the CLI, with the file saved as `hermes.yml`:

```sh
self-host apps add --name hermes --compose-file hermes.yml --web-port 9119
```

## Use it

Open `https://hermes.<suffix>` from any machine whose DNS points at the
Host. The browser asks for the dashboard username and password, then shows
the chat. The second Consumer does the same from their own browser.

The Platform's certificate is signed by its own CA. Each Consumer trusts it
once, using the fingerprint `self-host init` printed on the Host:

```sh
self-host trust-ca --from admin.<suffix> --fingerprint <sha256>
```

Accepting the browser warning instead works for a look, but not for a
Consumer who will use the chat every day.

## Where the data is

`~/.hermes` in the file lands in
`~/.config/self-host/apps/<id>/data/.hermes` on the Host, owned by the
container's user. Configuration, sessions and credentials live there and
survive:

- **Restart**: the Restart button, `self-host apps restart hermes`.
- **Container recreation**: editing the Compose file and saving, which is
  `docker compose up` again.
- **A Platform restart** and **a Host reboot**: the containers carry
  `restart: unless-stopped`, and the Platform republishes the route when it
  comes back.

Removing the Application keeps the directory. Delete it by hand to start
over.

## Change something

Edit the Compose file on the detail page and **Save and redeploy**. A change
to `environment` or `image` recreates the container; a change to the web
port only rewrites the route. Provider keys and dashboard credentials can
also be set outside the file with `self-host apps env set hermes KEY=value`,
which is rendered into the project on top of the file.

## When it does not work

- **`failed` with "service 'hermes' exited with code N"**: the container
  started and died. Read the logs pane; the last lines are Hermes's own
  reason. A missing dashboard password is the usual one.
- **The browser shows a Traefik 404 or 502**: the route is up but the
  service is not listening on 9119 yet. Wait for
  `HERMES_DASHBOARD_READY` in the logs. If it never appears, check that
  `HERMES_DASHBOARD=1` is set and the web port is 9119, not 8642.
- **The Hostname does not resolve**: the Consumer machine is not using the
  Host as its DNS server. See the DNS setup page in the console.
