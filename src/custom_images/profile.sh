# Keep login shells aligned with the image environment. Debian's /etc/profile
# rebuilds PATH, while direct processes inherit the ENV block in Dockerfile.
export MISE_INSTALL_PATH=/usr/local/bin/mise
export MISE_DATA_DIR=/opt/mise
export MISE_CONFIG_DIR=/opt/mise/config
export MISE_CACHE_DIR=/opt/mise/cache
export MISE_TRUSTED_CONFIG_PATHS=/opt/mise/config
export T3CODE_HOME=/data/t3home
export CARGO_HOME=/data/home/.cargo
export RUSTUP_HOME=/opt/mise/rustup
export PATH=/data/home/.cargo/bin:/opt/mise/cargo/bin:/opt/mise/shims:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
