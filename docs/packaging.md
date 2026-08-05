# Release packaging

PathPilot uses the application ID `io.github.RomanRcT.PathPilot`. The shared
desktop entry, AppStream metadata, and scalable icon live under `data/` and are
installed by `make install PREFIX=<prefix> DESTDIR=<root>`.

## RPM

The Fedora spec is `packaging/rpm/pathpilot.spec`. A release source archive must
be named `pathpilot-<version>.tar.gz` and contain a matching top-level directory.
The package builds with Cargo, installs under `/usr`, validates desktop metadata,
and lets RPM discover linked-library dependencies. Neovim is a recommendation
because only embedded editing requires it.

## Flatpak

The manifest is
`packaging/flatpak/io.github.RomanRcT.PathPilot.yml` and targets GNOME 50. Cargo
dependencies are vendored before `flatpak-builder` runs; the generated vendor
directory and `.cargo/config.toml` are intentionally not committed.

The application receives host-filesystem access for file management. Embedded
editing delegates to host Neovim through `flatpak-spawn --host`, preserving the
user's existing configuration and plugins.

## Automation

`.github/workflows/release-packages.yml` builds both formats on packaging pull
requests. Tag pushes and manual runs additionally generate `SHA256SUMS` and
upload the packages to the selected GitHub Release. Manual dispatch is used to
attach the first package set to the existing `v0.1.0` release without moving its
tag.
