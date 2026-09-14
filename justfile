setup-local:
    bash scripts/setup-local.sh

# The console is a built artifact the binary reads from console/dist, so the
# bundle goes first. A serve already holding :53, :443 and :80 has to go
# before the new one can bind them.
#
# Build the console, replace a running serve, and start the daemon.
run:
    cd console && npm run build
    -pkill -f 'self-host serve'
    cargo run -- serve
