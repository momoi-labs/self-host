# macOS CA trust over SSH

Investigation for [issue #78](https://github.com/momoi-labs/self-host/issues/78),
2026-09-08. The reported environment is macOS 26.5.2 on Mac16,7.
The Operator reproduced the authorization failure both over SSH with `sudo`
and through a root LaunchDaemon on macOS 26.5.2, build 25F84. Moving the
installation to a LaunchDaemon does not fix the issue on this build.
CLI TLS validation was also tested over SSH on the affected Mac.

## Findings

Apple introduced an explicit administrator authorization requirement for
changes to the admin trust domain in Big Sur. Root privileges alone no longer
suffice. Apple names both root scripts calling `security add-trusted-cert -d`
and root processes calling `SecTrustSettingsSetTrustSettings` as affected.
This matches the authorization failure reported in the issue.
[macOS Big Sur 11.0.1 release notes, Security](https://developer.apple.com/documentation/macos-release-notes/macos-big-sur-11_0_1-release-notes/)

In October 2025, Apple DTS confirmed that programmatically installing a root
certificate with system-wide trust without explicit user approval has no
supported solution. DTS described this as deliberate security hardening and
recommended an MDM-delivered `com.apple.security.root` payload for managed
devices. DTS also cautioned that shell workarounds depend on implementation
details. Earlier in the same thread, DTS warned that allowing
`com.apple.trust-settings.admin` through the authorization database lets any
code on the machine add trusted roots.
[Apple DTS discussion](https://developer.apple.com/forums/thread/671582)

The root LaunchDaemon experiment below returned the same authorization error
as direct SSH execution. In both cases, the temporary CA remained untrusted.
This confirms the failure for the tested macOS build and execution contexts.

In the repository, [`install_ca`](../../src/main.rs) invokes the affected
command on macOS. Both the local `trust-ca` path and
`trust-ca --from ... --fingerprint ...` call it. The latter downloads and
verifies the CA before installing it on the machine running the command.
Consumer trust does not depend on the Host trusting its own CA, but a macOS
Consumer still needs authorization for its own System Keychain.

The Host impact also extends to the CLI. `run_apps_command` and
`run_logs_command` create default reqwest clients, while `cli_config` selects
`https://admin.<suffix>`. [`Cargo.toml`](../../Cargo.toml) enables
`rustls-tls-native-roots`. This implies those commands can fail TLS validation
when the Host has not trusted its CA. The SSH test below reproduced an
`apps list` TLS failure with an empty client trust store and a successful
request when given the local CA. `logs` was inspected but not exercised.

## Recommended direction

Keep Host CA trust optional during installation. Explain that retrying over
SSH does not provide the missing authorization. For an unmanaged Mac, direct
the Operator to run `self-host trust-ca` in Terminal in the Mac's graphical
session and approve the system prompt. Phrase this as graphical authorization,
not an unconditional requirement for physical access. Managed installations
can distribute the CA through MDM, as Apple recommends.

For headless Host operation, add the existing local Platform CA from
`tls::ca_cert_path()` to the CLI clients used by `apps` and `logs`. That would
let these commands validate the Platform's certificate without changing
system trust. Preserve certificate and hostname verification. Validate this
change against a local TLS server and the affected Mac before claiming it
fixes the operational failure. Browsers would still need OS trust.

A verified workaround for the current CLI is a command-scoped CA file.
The dependency `rustls-native-certs` 0.8.4 checks `SSL_CERT_FILE` before loading
the platform store. This replaces the roots for that invocation while keeping
TLS validation enabled. The SSH validation below confirms this behavior.
[Dependency source](https://docs.rs/rustls-native-certs/0.8.4/src/rustls_native_certs/lib.rs.html)

```sh
SSL_CERT_FILE="$HOME/.config/self-host/certs/ca.pem" self-host apps list
```

Do not add the proposed LaunchDaemon fallback. It failed in the tested
environment. Update the installer warning and Host documentation to describe
the graphical authorization requirement and the separate CLI workaround.

## SSH validation on 2026-09-08

Connected to the Host at `192.168.1.100` as the Operator. It reports macOS
26.5.2, with the binary at `/usr/local/bin/self-host`.

Both the default `self-host apps list` and the invocation using the local CA
returned `No Applications deployed.` with exit status 0. At test time,
`security dump-trust-settings -d` listed `Self-Host LAN CA`, and
`security verify-cert -c "$HOME/.config/self-host/certs/ca.pem" -p basic`
reported successful verification. The original untrusted state was absent.

To check the CLI workaround independently of existing system trust, ran:

```sh
SSL_CERT_FILE=/dev/null SSL_CERT_DIR=/var/empty /usr/local/bin/self-host apps list
SSL_CERT_FILE="$HOME/.config/self-host/certs/ca.pem" SSL_CERT_DIR=/var/empty /usr/local/bin/self-host apps list
```

The empty-root invocation failed with `invalid peer certificate: UnknownIssuer`
and exit status 1. Supplying the local CA succeeded with exit status 0.
This verifies the CLI can use the local CA without relying on System Keychain
trust. It does not reproduce the authorization failure of `trust-ca`.

The Operator later repeated the command with the local CA and confirmed that
it listed the `hermes` Compose Application as `running`.

This CLI test did not change system trust settings. The Operator subsequently
ran the installation experiment below with interactive `sudo` authentication.

## Root LaunchDaemon validation on 2026-09-08

The Operator ran a temporary probe script through SSH on macOS 26.5.2,
build 25F84. The script used a new, self-signed CA with a unique name and a
one-day lifetime. It left the existing Platform CA alone.

The probe first verified the temporary CA with an explicit root argument,
then confirmed that verification without that argument failed. This checked
that the certificate was valid and initially untrusted.

It then ran the same command in two contexts:

```sh
/usr/bin/security add-trusted-cert -d -r trustRoot \
  -k /Library/Keychains/System.keychain "$PROBE_DIR/ca.pem"
```

The first invocation ran as root under `sudo` in the SSH session. After it
failed and the CA remained untrusted, the second ran in a temporary system
LaunchDaemon with `UserName=root` and `RunAtLoad=true`, loaded with
`launchctl bootstrap system`. Neither invocation timed out. Both exited with
status 1 and reported:

```text
SecTrustSettingsSetTrustSettings: The authorization was denied since no user interaction was possible.
```

After each invocation, `security verify-cert -c "$PROBE_DIR/ca.pem" -p basic
-l -L` still failed without an explicit trust anchor. The final results were:

```text
SSH_RESULT=exit-1,untrusted
LAUNCHDAEMON_RESULT=exit-1,untrusted
CLEANUP_RESULT=complete
```

The script unloaded the temporary daemon, removed the temporary certificate,
deleted its private key and temporary files, and checked that the test CA
was absent and untrusted. During cleanup, `SecTrustSettingsRemoveTrustSettings`
reported that the item could not be found because no trust settings had been
installed. This did not prevent certificate removal or the cleanup checks.

The Operator supplied the completed probe output. A subsequent SSH read of
the saved report confirmed these results. The temporary directory was absent,
and verification of the existing Platform CA still succeeded. This establishes
a failed LaunchDaemon fallback on build 25F84; it does not establish behavior
on every macOS version.
