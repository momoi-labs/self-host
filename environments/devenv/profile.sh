# The container's environment, again, for shells that rebuild their own.
# /etc/profile resets PATH from scratch, so a login shell would otherwise see
# none of the toolchains. Keep this in step with the ENV block in Dockerfile.
export MISE_TRUSTED_CONFIG_PATHS=/etc/mise:/mise
export T3CODE_HOME=/data/t3home
export CODEX_HOME=/data/home/.codex
export CARGO_HOME=/data/home/.cargo
export NPM_CONFIG_PREFIX=/data/home/.npm-global
export PATH=/data/home/.npm-global/bin:/data/home/.cargo/bin:/mise/shims:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
