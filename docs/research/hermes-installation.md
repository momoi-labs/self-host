# Hermes installation requirements

Research recorded on September 5, 2026. No installer was executed and no Mac
deployment was tested. Installer source was inspected at commit
`52cf39c908eed3a545ece5a180425b1f34b96829`; upstream requirements may change.

The approved [MVP](../vision.md) uses Docker Compose. Native installation research
remains a reference for future capabilities, not an additional MVP requirement.

## Docker path selected for the MVP

The official `nousresearch/hermes-agent` image accepts `setup` and `gateway run`,
receives environment variables and persists data at `/opt/data`. Its internal
s6-overlay supervision does not require s6 on the Host. The documentation includes
a Compose example with a persistent mount, restart policy and optional dashboard.
Browser and API access still require configuration.
[Official Docker guide](https://hermes-agent.nousresearch.com/docs/user-guide/docker).

Validate the image on Apple Silicon, its initial configuration workflow, model
authentication and the browser chat before treating this path as complete.
Preserving configuration and conversation data requires persistent storage across
container recreation. A running container alone does not demonstrate usable chat.

## What the native installer teaches us

Apple Silicon macOS is listed as a primary supported platform. The curl-based
installation downloads a shell script that obtains source and dependencies,
rather than a standalone binary. Read it to identify requirements; the product
decision is not to delegate installation to that script.
[Platform support](https://hermes-agent.nousresearch.com/docs/getting-started/platform-support),
[installer source](https://github.com/NousResearch/hermes-agent/blob/52cf39c908eed3a545ece5a180425b1f34b96829/scripts/install.sh).

| Observed installation step | Candidate Platform capability |
| --- | --- |
| Clone source and select a revision | Obtain an Application's code or package |
| Prepare Python, a virtual environment, Node and dependencies | Prepare runtimes using existing tools |
| Install packages and expose the Hermes command | Run Application-specific installation steps |
| Set directories, configuration and credentials | Preserve data and supply configuration |
| Keep a gateway or backend running | Start and supervise the required processes |
| Make the chosen interface usable | Validate the Consumer's actual workflow |

The capability column is a product inference from the installer, not a commitment
to implement every row now. mise was considered as a helper; it is not selected
as an MVP prerequisite.

## Native installation details

The inspected script clones the repository, prepares uv, Python 3.11 and a virtual
environment, installs dependencies and exposes `hermes`. It also prepares Node
and helper tools. Docker is not required by this native path.

For a per-user installation, source lives under `~/.hermes/hermes-agent/`, the
command under `~/.local/bin/hermes` and data under `~/.hermes/`. `HERMES_HOME` can
override the data directory.
[Installation and data layout](https://hermes-agent.nousresearch.com/docs/getting-started/installation).

The script supports `--skip-setup`, `--skip-browser`, `--skip-computer-use` and
commit pinning. Without a terminal it skips the wizard. Separating installation
from configuration does not supply provider credentials automatically.
[Pinned installer](https://github.com/NousResearch/hermes-agent/blob/52cf39c908eed3a545ece5a180425b1f34b96829/scripts/install.sh).

## Gaps on a relatively clean Mac

The script attempts to obtain Git and a compiler through Apple's developer
tools, which can open an installation dialog. It tries Homebrew for optional
ffmpeg and ripgrep dependencies and can continue with limitations if unavailable.
Its final gateway startup uses `nohup` when systemd is absent; that does not
install a macOS system service.
[Installer functions](https://github.com/NousResearch/hermes-agent/blob/52cf39c908eed3a545ece5a180425b1f34b96829/scripts/install.sh).

Separately, `hermes gateway install` creates a launchd LaunchAgent using
`RunAtLoad` and `KeepAlive`. A per-user agent is not evidence of startup before
login.
[Gateway implementation](https://github.com/NousResearch/hermes-agent/blob/52cf39c908eed3a545ece5a180425b1f34b96829/hermes_cli/gateway.py).

Reboot without login, logout, sleep and closed-lid behavior remain untested on
the actual Host. This research did not measure RAM, storage consumption or local
model performance on the M4.

## Configuration and persistence

The Operator chooses a provider, model and tools. Provider authentication or a
custom endpoint may be used; running a local model is not required.
[Quickstart](https://hermes-agent.nousresearch.com/docs/getting-started/quickstart/).

Configuration, sessions and credentials live in the data directory. Hermes has
backup/import capabilities; profile-only export excludes credentials. These facts
inform persistence requirements without adding Platform backup support to the MVP.
[Installation guide](https://hermes-agent.nousresearch.com/docs/getting-started/installation).

1Password integration is not required; see the separate
[optional research](onepassword-secrets.md).

## Access from another machine

| Interface | Documented requirements |
| --- | --- |
| Messaging bot | Gateway plus channel-specific tokens and allowed users |
| Remote desktop client | A reachable backend and authentication; the guide uses `hermes serve` and port 9119 |
| Browser | A dashboard with chat; native installation documentation lists web/PTY extras and a frontend build where needed |

Sources: [messaging gateway](https://hermes-agent.nousresearch.com/docs/user-guide/messaging),
[remote desktop backend](https://hermes-agent.nousresearch.com/docs/user-guide/desktop#connecting-to-a-remote-backend)
and [web dashboard](https://hermes-agent.nousresearch.com/docs/user-guide/features/web-dashboard).

The desktop guide refers to `hermes serve`, while the dashboard guide describes
`hermes dashboard` for a remote backend. Do not assume they are interchangeable
without a test. Both distinguish that backend from the messaging gateway.
The approved MVP uses browser chat through the Docker workflow.

## Voice requirements are deferred

Audio messages in Telegram or Discord differ from live Discord voice channels.
CLI voice uses the microphone and audio output of the machine running the CLI.
ffmpeg handles conversion, PortAudio is needed for CLI voice and Opus for Discord
voice channels. Speech recognition and synthesis require configured providers,
which can be local or remote.
[Voice documentation](https://hermes-agent.nousresearch.com/docs/user-guide/features/voice-mode).

The required dependencies depend on the chosen interaction mode. Browser chat is
the first use case; this research does not justify installing every voice
dependency in advance.
