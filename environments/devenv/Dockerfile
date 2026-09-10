# The development environment for one person: T3 Code, the agent CLIs and the
# toolchains this repository and pg-probe need.
#
# The image holds tools and never credentials, because a credential baked here
# would be shared by everyone who runs it. Everything personal lives on the
# volume this image expects at /data (issue #89).
#
# The Platform refuses `build:` in a Compose Application, so this is built by
# hand on the Host and the Compose file beside it refers to the tag. See
# README.md.
#
# The base is mise's own image: Debian 13 on the official Rust image, with
# mise, its shims on PATH, Rust, Node and Python already there.
FROM jdxcode/mise:2026.9.4

ARG T3_VERSION=0.0.40
ARG CLAUDE_CODE_VERSION=2.1.267
ARG CODEX_VERSION=0.154.0
ARG OPENCODE_VERSION=1.18.30
ARG RUST_VERSION=1.97.1
ARG USERNAME=dev
ARG USER_UID=1000

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# The Host is an Apple Silicon Mac and its VM is aarch64. Saying so here turns
# a wrong architecture into a failed build instead of a container that starts
# and then cannot run its own toolchain.
RUN test "$(dpkg --print-architecture)" = arm64 \
	|| { echo "this image is arm64 only; got $(dpkg --print-architecture)" >&2; exit 1; }

# Rust rides on the base image, so a base bump can move it without anything
# here changing. Both repositories pin the version, so say what is expected
# and fail the build rather than discover it in a compile error.
RUN test "$(rustc --version | cut -d' ' -f2)" = "${RUST_VERSION}" \
	|| { echo "the base image carries rustc $(rustc --version | cut -d' ' -f2), not ${RUST_VERSION}" >&2; exit 1; }

RUN apt-get update && apt-get install -y --no-install-recommends \
	fzf \
	gosu \
	jq \
	less \
	libssl-dev \
	locales \
	pkg-config \
	ripgrep \
	rsync \
	sqlite3 \
	sudo \
	tmux \
	unzip \
	zip \
	&& rm -rf /var/lib/apt/lists/* \
	&& sed -i 's/^# en_US.UTF-8/en_US.UTF-8/' /etc/locale.gen && locale-gen

# A person, not root. Their home is the volume, declared here and not only in
# ENV: gosu resets HOME to whatever /etc/passwd says, and a login shell agrees
# with it, so anything else would send state somewhere a recreate destroys.
# Passwordless sudo because the container boundary is the isolation here, and
# an apt install inside it is gone on the next recreate anyway.
RUN useradd --uid "${USER_UID}" --no-create-home --home-dir /data/home --shell /bin/bash "${USERNAME}" \
	&& echo "${USERNAME} ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/"${USERNAME}" \
	&& chmod 0440 /etc/sudoers.d/"${USERNAME}"

# clippy and rustfmt are what CI runs; the musl targets are what pg-probe
# releases. rust-toolchain.toml would install them on first use, once per
# container, which is a slow surprise on a Host nobody is watching.
RUN rustup component add clippy rustfmt \
	&& rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

# Everything else comes from mise, so this file is the whole list and adding a
# tool is a line rather than a download recipe. The shims work without a shell
# to activate them, which is what T3 Code spawns processes with.
# /etc/mise/config.toml is mise's system config, which it merges with the
# base image's own. Appending to that one instead would give it two [tools].
#
# Trust is per user state, and this image is built by root and run by someone
# else, who would find every config ignored and every shim answering "No
# version is set". Naming the paths is what survives that change of user.
ENV MISE_TRUSTED_CONFIG_PATHS=/etc/mise:/mise
COPY mise.toml /etc/mise/config.toml
RUN mise install --yes \
	&& mise reshim \
	&& rm -rf /mise/downloads

RUN npm install -g --no-fund --no-audit \
	"t3@${T3_VERSION}" \
	"@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
	"@openai/codex@${CODEX_VERSION}" \
	"opencode-ai@${OPENCODE_VERSION}" \
	&& npm cache clean --force \
	&& mise reshim

# mise and rustup install into the image at runtime too, for a project that
# pins something this image does not carry. That belongs to the container and
# not to the volume, so it is gone on the next recreate: pin it here instead.
RUN chown -R "${USERNAME}:${USERNAME}" /mise /usr/local/rustup /usr/local/cargo

# A login shell rebuilds PATH from /etc/profile and would find none of the
# above. The terminal T3 Code opens is a login shell.
COPY profile.sh /etc/profile.d/devenv.sh

COPY entrypoint.sh /usr/local/bin/devenv-entrypoint
COPY first-boot.sh /usr/local/bin/devenv-first-boot
RUN chmod 0755 /usr/local/bin/devenv-entrypoint /usr/local/bin/devenv-first-boot

# Everything that must survive a recreate resolves under /data, and nothing
# else does. HOME is the volume, so shell history, agent credentials, the SSH
# key and the Cargo cache all land there without being listed one by one.
#
# CARGO_HOME moves to the volume but RUSTUP_HOME does not: the toolchain is
# the image's to replace, and only the crate cache and `cargo install` are
# worth carrying between containers.
ENV DEVENV_USER=${USERNAME} \
	HOME=/data/home \
	T3CODE_HOME=/data/t3home \
	CODEX_HOME=/data/home/.codex \
	CARGO_HOME=/data/home/.cargo \
	NPM_CONFIG_PREFIX=/data/home/.npm-global \
	LANG=en_US.UTF-8 \
	PATH=/data/home/.npm-global/bin:/data/home/.cargo/bin:/mise/shims:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin

WORKDIR /data/repos
EXPOSE 3773

# Root only long enough to hand the volume to the user; see entrypoint.sh.
ENTRYPOINT ["/usr/local/bin/devenv-entrypoint"]
