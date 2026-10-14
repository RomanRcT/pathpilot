# Release packaging

PathPilot uses the application ID `io.github.RomanRcT.PathPilot`. The shared
desktop entry, AppStream metadata, and scalable icon live under `data/` and are
installed by `make install PREFIX=<prefix> DESTDIR=<root>`.

## RPM

The Fedora spec is `packaging/rpm/pathpilot.spec`. A release source archive must
be named `pathpilot-<version>.tar.gz` and contain a matching top-level directory.
The package builds with Cargo, installs under `/usr`, validates desktop metadata,
and lets RPM discover linked-library dependencies. The release workflow repacks
the verified vendored offline source as this RPM build archive. Neovim is a recommendation
because only embedded editing requires it. WebKitGTK 6.0 is a build and runtime
dependency for Markdown HTML preview.

## DEB

`packaging/debian/` contains a debhelper 13 recipe. The Ubuntu 24.04 amd64 build
uses the same verified offline source archive as RPM and Flatpak. Run
`python3 tools/build-deb.py <fresh-offline-source-tree> --output release-assets`
with Rust 1.90+, debhelper, dpkg-dev, fakeroot and the native development headers
installed. The helper generates a Debian changelog from the source Cargo version.
It skips dpkg's build-dependency database check because CI uses rustup; native
build packages are explicitly installed by the workflow.

Debhelper generates shared-library dependencies from the actual ELF binary;
minimum GTK/libadwaita versions are also declared explicitly. `7zip` is required;
Poppler and GVfs backends are recommended, LibreOffice and Neovim are optional.
Desktop metadata, AppStream and license notices are included. The build validates
metadata, checks linked libraries and installs the finished package with apt.
Ubuntu 24.04 is the tested DEB target; Debian 13 is a compatibility candidate.
Older GTK distributions require Flatpak.

`.github/workflows/deb-package.yml` is reusable by the main release workflow.
A standalone dispatch with an existing release tag downloads its published
source archive, builds only the DEB and appends it to that release's SHA256SUMS.
This permits adding a DEB to v0.4.0 without moving its tag or rebuilding RPM/Flatpak.

## Flatpak

The manifest is
`packaging/flatpak/io.github.RomanRcT.PathPilot.yml` and targets GNOME 50. Cargo
dependencies are vendored before `flatpak-builder` runs; the generated vendor
directory and `.cargo/config.toml` are intentionally not committed. WebKitGTK
6.0 is supplied by the GNOME 50 SDK/runtime.

The application receives host-filesystem access for file management. Embedded
editing delegates to host Neovim through `flatpak-spawn --host`, preserving the
user's existing configuration and plugins.

## Automation

`.github/workflows/release-packages.yml` builds all three formats on packaging pull
requests. Tag pushes and manual runs additionally generate `SHA256SUMS` and
upload the packages to the selected GitHub Release. Manual dispatch is used to
rebuild or replace package assets for an existing tag without moving the tag.
The workflow verifies that the tag, Cargo workspace version, RPM spec, and
latest AppStream release agree before starting either package build.
