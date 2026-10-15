# The console shows the Mac's screen through Screen Sharing

A Mac Host still asks for clicks the Platform cannot give: privacy prompts,
Full Disk Access, the first run of a setup wizard. Walking to the machine for
each one is the cost this decision removes.

The console gets a Host screen. The Platform does not capture the screen
itself and does not run its own VNC server. macOS Screen Sharing already is
one, on `127.0.0.1:5900`, and the Platform only carries bytes between the
console's WebSocket and that port. The console draws them with noVNC.

- **Off by default.** A setting, `remoteScreen`, allows it. While it is off,
  the WebSocket closes before the Platform opens the port.
- **Two locks.** The WebSocket authenticates its first frame with the API
  key, like the terminal (a browser cannot set a header on an upgrade). Then
  Screen Sharing asks for a Mac account itself. The Platform never sees or
  keeps that password.
- **The Operator turns Screen Sharing on.** It needs an administrator, and
  the Platform has no sudo. The console says when it does not answer.
- **macOS only.** A Linux Host would need its own VNC server; that is a later
  decision.

What this does not solve, and the console cannot detect: a MacBook with its
lid closed and no display attached shows a black screen, even with a virtual
display. Screen Sharing needs an open lid, a monitor, or an HDMI dummy plug.

The Mac's sign-in uses WebCrypto, which browsers only allow on a secure page,
so the screen works from the HTTPS console and not from `http://<host>:3721`.

Status: accepted.

## Amendment: a shell on the Host

The Host page also opens a shell on the Host, behind its own setting,
`hostTerminal`, off by default. It is a login shell in the home directory of
the account the Platform runs under, on a pseudo-terminal, the same way the
console opens an Application's terminal. It can do anything that account can,
which is why it has a switch of its own:

- On a Mac that account is the Operator's, and it may run the `native-control`
  command the installer allows without a password.
- On a Linux Host that runs the Platform as root, such as a VPS, it is a root
  shell. The console names the account before the shell starts.

Unlike the screen, the shell is not limited to macOS: it needs nothing from
the Host but a pseudo-terminal. The screen then opens in a window of its own,
so the shell keeps the page.
