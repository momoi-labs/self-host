# Releases

Changesets keeps one version PR open. Merging it tags the version and runs
GoReleaser, which publishes GitHub assets and the Homebrew Cask. Linux packages
for pacman, apt, and dnf are attached to each release. AUR publication is
disabled. Nothing is published to npm.

## First release

The initial changeset includes the changes since `v0.3.0` and proposes
`v0.4.0`. The root `package.json` starts at the last published version,
`0.3.0`. Cargo already says `0.4.0`; the first version PR brings them together.
Later releases update both manifests in that PR.

Complete the package registry setup below before merging the first version PR.
Merging the implementation PR only opens the version PR.

## Registry setup

1. Use the public GitHub repository `momoi-labs/homebrew-packages` with an initial
   commit, such as a README. GoReleaser will maintain `Casks/self-host.rb` there.
2. Create a fine-grained GitHub token restricted to that repository, with
   **Contents: Read and write**. Store it as the organization Actions secret
   `HOMEBREW_TAP_TOKEN`, with access granted to `momoi-labs/self-host`.
3. In this repository's **Settings > Actions > General**, enable **Allow GitHub
   Actions to create and approve pull requests**. The version workflow uses the
   built-in `GITHUB_TOKEN` with explicit workflow permissions.

The version workflow explicitly dispatches CI on its PR branch and calls the
release workflow after tagging. GitHub does not trigger these workflows from
events created with `GITHUB_TOKEN`.

Stable releases require `HOMEBREW_TAP_TOKEN`. Snapshots and
prereleases do not update Homebrew. No AUR key or account is required.
GoReleaser still generates the AUR recipe under `dist/aur/`, with upload disabled.

## Add release notes

From the repository root:

```sh
npm ci
npm run changeset
```

Select `self-host`, choose the version bump, and describe the user-visible
change in English. Commit the generated `.changeset/*.md` with the change.
Use a patch for fixes and a minor for new behavior. Changesets combines pending
entries into the next version PR. Documentation-only changes need no changeset.

On `main`, the Version workflow opens or updates `chore: release self-host`.
Review its changelog and version, wait for CI, then merge it. The PR updates
`package.json`, `package-lock.json`, `Cargo.toml`, and `Cargo.lock` together.
Do not bump these versions by hand.

The release workflow extracts that version's summary from `CHANGELOG.md` and
passes it to GoReleaser with `--release-notes`. GoReleaser publishes it as the
GitHub release description, followed by a link to the full comparison between
tags. Keep `changelog.disable` set to `false`: disabling the changelog also
prevents GoReleaser from reading the supplied notes.

## Release outputs

The Release workflow builds four targets in parallel, each on its own runner.
Each job builds the console and one Rust binary. Apple targets use macOS;
Linux targets use Ubuntu with Zig. A final job downloads those binaries and
runs GoReleaser to package and publish them without compiling again.

CI sets `SELF_HOST_PREBUILT_DIR` to the downloaded artifacts. The Rust tool hook
in `scripts/release-cargo.sh` imports each binary and fails if it is missing.
Local GoReleaser runs use Cargo when that variable is unset.

GoReleaser produces:

- Four tarballs, named `self-host_v<version>_<rust-target>.tar.gz`, with the
  binary and MIT license. Existing `install.sh` URLs keep working.
- Two packages per Linux format, for amd64 and arm64: `.pkg.tar.zst`, `.deb`,
  and `.rpm`. Each format uses its distribution's architecture and dependency names.
- `checksums.txt` with SHA-256 hashes of the downloadable packages and archives.
- A Homebrew Cask with both macOS architectures and their SHA-256 hashes.
- A local AUR `self-host-bin` recipe with both Linux architectures and their
  hashes. It is not pushed to the AUR.

The Cask is macOS-only. It installs an unsigned, non-notarized binary and removes
the quarantine attribute from that binary in its install hook, as described in
the [GoReleaser Cask documentation](https://goreleaser.com/customization/publish/homebrew_casks/).
The package does not configure Docker, DNS, privileges, or daemon supervision.
Uninstalling removes the binary; it preserves Platform State.

## Retry or build a snapshot

In **Actions > Release > Run workflow**, use an existing tag
to retry a failed release. The workflow checks out that tag, checks the version,
and uses its changelog. Re-running the Version workflow alone will not retry a
tag it has already created.

For tags before the parallel artifact workflow, including `v0.4.0`, re-run the
original release run instead. Those tags have the older GoReleaser configuration.

Leave the input as `snapshot` to replace the moving snapshot prerelease from
the selected branch. It includes tarballs, Arch, Debian, RPM packages, and checksums, but
does not update package registries or become GitHub's latest stable release.

## Verification

Run these checks before merging release changes:

```sh
npm ci
npm run test:release
goreleaser check
actionlint
```

The version tests exercise Changesets in a temporary Git repository, check Cargo
synchronization, and verify that tagging a version twice does not publish twice.
The artifact tests verify that publishing preserves each target's binary and
rejects missing or non-executable artifacts.
On macOS with Rust, Zig 0.13.0, cargo-zigbuild, and GoReleaser 2.18.2 installed,
build all packages without publishing:

```sh
(cd console && npm ci && npm run build)
HOMEBREW_TAP_TOKEN='' goreleaser release --snapshot --clean
```

After the first stable release, verify both architectures where available:

1. On macOS, run `brew install --cask momoi-labs/packages/self-host`, then
   `self-host --help`. Run `brew uninstall --cask self-host` and confirm that
   Homebrew removed its `self-host` link.
2. Download a release's Linux package and `checksums.txt`. Verify its hash
   with `sha256sum --check --ignore-missing checksums.txt`, then install it with
   the command for your distribution in the README.
3. Run `self-host --help` and query the installed package with
   `pacman -Q self-host-bin`, `dpkg-query -W self-host-bin`, or
   `rpm -q self-host-bin`. Remove it with your package manager and confirm
   `/usr/bin/self-host` is gone.
4. For a configured Host, stop its existing daemon, start `self-host serve`,
   open its console, and click **Applications**. The list should load. Click
   an Application to verify its details still load after the package upgrade.
