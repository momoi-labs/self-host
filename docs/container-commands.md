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

## Validation

User testing confirmed the terminal and its empty-state layout. Browser checks
covered starting on click, container selection, paste, Ctrl+C, tab switching,
closing and reopening. A real development container kept shell state, resized
its PTY and ran as `dev`. Disconnecting ended its shell.

The console build, two console tests, 224 Rust tests, formatting, clippy and
the all-targets check passed. WebSocket tests cover authentication, container
ownership, stopped containers, dimensions and user selection.
