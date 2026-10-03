# Invoked only after N1 has dropped privileges and joined the app cgroup.
umask 077
config=$1
install=$2
shift 2
mkdir -p "$MISE_CONFIG_DIR" "$MISE_DATA_DIR" "$MISE_CACHE_DIR" "$MISE_STATE_DIR" \
  "$MISE_TMP_DIR" "$MISE_SYSTEM_CONFIG_DIR" "${MISE_INSTALL_PATH%/*}" \
  "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" \
  "$CARGO_HOME" "$RUSTUP_HOME"
printf '%s' "$config" > "$MISE_CONFIG_DIR/config.toml.new"
mv -f "$MISE_CONFIG_DIR/config.toml.new" "$MISE_GLOBAL_CONFIG_FILE"
if [ "$install" = 1 ]; then
  if [ ! -x "$MISE_INSTALL_PATH" ]; then
    printf 'Installing mise under the Application Account.\n'
    /usr/bin/curl --proto '=https' --proto-redir '=https' --fail --silent --show-error \
      --location --connect-timeout 15 --max-time 120 https://mise.run \
      --output "$MISE_TMP_DIR/install-mise.sh"
    MISE_QUIET=1 /bin/sh "$MISE_TMP_DIR/install-mise.sh"
    rm -f "$MISE_TMP_DIR/install-mise.sh"
  fi
  printf 'Installing configured dependencies.\n'
  "$MISE_INSTALL_PATH" install --yes
  "$MISE_INSTALL_PATH" reshim
  step=0
  for command in "$@"; do
    step=$((step + 1))
    printf 'Running non-root setup command %s.\n' "$step"
    "$MISE_INSTALL_PATH" exec -- /bin/sh -ec "$command"
  done
fi
printf 'Native environment ready.\n'
