# Open a container terminal

Open an Application and select **Terminal** beside **Logs**. Click **Start
terminal**. Applications with multiple containers also show a container selector.
Starting the session replaces the empty state with the terminal. The Platform
opens `docker exec -it` with `bash -i`. Bash must already be installed in the image.

The terminal supports interactive prompts, paste, Ctrl+C, terminal resizing,
and commands that keep shell state, such as `cd` and `export`. Switching between
Logs and Terminal keeps the session open. Output stays in the browser, with a
5,000-line scrollback limit.

Click **Close terminal**, run `exit`, or leave the Application page to end the
session. A lost connection ends the session too. Start another terminal to
reconnect. Closing the terminal does not stop the Application.

Development images use `dev`, with the image's HOME and mise PATH. Other images
use their configured container user. The terminal does not install packages or
change the Application's start command.

## Transport and cleanup

`/apps/id/{id}/terminal` upgrades to a WebSocket. Its first frame supplies the
API key, container, columns, and rows. The server authenticates within five
seconds and checks Application ownership and running state before opening a
PTY. Credentials never appear in the URL. Use the console's HTTPS address.

Binary frames carry PTY output. JSON frames carry input, resize events, and
session status. Ping frames detect broken connections. The Host runs the Docker
CLI in a PTY so Bash has job control inside the container.

Each exec receives a random session marker. On disconnect, the Platform sends
SIGHUP to container processes carrying that marker, without stopping other exec
sessions or the Application. Processes that deliberately ignore SIGHUP can
continue running. Restarting the Platform abruptly cannot guarantee cleanup.

## Access review

Nothing starts before the request proves who it is and what it owns.

The route sits outside the API key middleware because a browser cannot set a
Bearer header on a WebSocket upgrade. The handler takes its place: the first
text frame must arrive within five seconds and carry a key that matches the
stored one in constant time. A Host with no key stored refuses every request,
so an uninitialised Platform cannot be opened. The key is never in the URL, so
it stays out of access logs and `Referer` headers.

The upgrade does not check `Origin`, and WebSockets are not covered by the
same-origin policy, so any page can open this socket. None can authenticate:
the key lives in `sessionStorage` on the console's origin, which a cross-origin
page cannot read. The exposure is one unauthenticated socket held for at most
five seconds. Add an `Origin` check if the console ever keeps its key somewhere
a cross-origin page can reach.

After the key, the handler resolves the Application and requires the container
to be one of its own and to be running. A container belonging to another
Application is refused by name, before `docker exec` runs. The user is the
Platform's to choose, not the request's: `dev` for a development image, the
image's configured user otherwise. There is no field that asks for root.

Frames are capped at 64 KiB, an input payload at 16 KiB, columns at 2 to 500
and rows at 1 to 300. Anything else closes the socket. A ping every fifteen
seconds and a forty-five second idle limit close abandoned ones, and dropping
the session kills its PTY.

What the review does not remove: a terminal is exactly as privileged as the
container user, and an API key is now enough to reach a shell inside every
Application. An Operator who can reach the console could already deploy an
Application, so this grants no new reach on the Host, but it does change what
handing out an API key means. Worth saying where keys are created.

## Validation

User testing on the original branch confirmed the terminal and its empty-state
layout. Browser checks covered starting on click, container selection, paste,
Ctrl+C, tab switching, closing and reopening. A real development container kept
shell state, resized its PTY and ran as `dev`. Disconnecting ended its shell.

After the transplant onto current main: 239 Rust tests, 12 console tests, the
console build, formatting, clippy and the all-targets check passed. The
WebSocket tests cover authentication, container ownership, stopped containers,
dimensions and user selection. Browser checks against a stand-in PTY bridge
covered the connect frame, the ready and output frames, typing, resizing and
closing without a spurious failure.

The Docker exec path was then re-run against a real development container on
the transplanted branch. `whoami` answered `dev`, `export` and `echo` kept
shell state across commands, and `/etc/os-release` read Debian 13 from inside
the Application rather than from the Host.
