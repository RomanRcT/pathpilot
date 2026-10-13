# Document engine dependencies and updates

WordCraft (DOCX) and GridCraft (XLSX) use exact full Git commit revisions in
`Cargo.toml`, recorded again in `Cargo.lock`. All modules of one engine must
use the same revision. Upstream changes do not affect existing builds or the
installed application until a reviewed update is merged.

## Enable automatic update proposals

Install the [Renovate GitHub App](https://github.com/apps/renovate/installations/new)
for **only the PathPilot repository**. The committed `renovate.json` enables
weekly checks before 06:00 Monday in America/Chicago. It groups updates into
one PR per engine, with at most two concurrent PRs. Automatic merging and
Dependency Dashboard issues are disabled. Other Cargo dependencies are outside
this configuration's scope.

Renovate proposes revisions and lockfile changes; maintainers review and merge
PRs. Connecting the app is a separate repository-owner action. Until connected,
no automated update PRs will be created.

## Review an engine update

1. Review upstream code, compatibility and license changes. Confirm that all
   modules of the engine have the same full revision and the lockfile matches.
2. Run `python3 tools/document-licenses.py`. CI compares the checked-in notices
   byte for byte with the pinned source. If they changed, inspect the changes,
   then explicitly run `python3 tools/document-licenses.py --update` and commit
   the reviewed snapshots. This command needs the original Git sources, before
   generating a vendored checkout. `--offline` uses cached sources.
3. Run `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, and
   `cargo test --workspace`. Run document integration tests with
   `cargo test -p pathpilot-preview -- --ignored` (Poppler and LibreOffice required).
4. Open representative DOCX/XLSX files, including styled tables, merges and
   multi-page documents, with `cargo run -- --new-instance`. Check appearance,
   navigation, and fallback previews. Passing compilation does not establish
   rendering fidelity.
5. Generate and verify the offline source archive below. Release-package PR
   checks also compile it with a fresh Cargo cache and build RPM/Flatpak packages.

The engine code is used under the MIT option of its MIT/Apache-2.0 dual license.
Original copyright, license and attribution files are retained in
`data/licenses/wordcraft` and `data/licenses/gridcraft`, together with bundled
font licenses and applicable hyphenation notices. Packages install these files;
About also credits and links the engines and displays their MIT license texts.
Review new bundled resources and their licenses on updates as well.

## Preserve independent build inputs

```sh
python3 tools/create-offline-source.py --output release-assets
python3 tools/verify-offline-source.py release-assets/pathpilot-0.3.0-offline-source.tar.gz --release
```

Use the current package version in the filename. The creator downloads locked
crates as necessary; pass `--offline` if every dependency is already cached.
The archive includes registry and Git dependencies, compile-time assets,
notices, Cargo source replacement configuration, and a `SOURCE-MANIFEST.json`
with exact revisions and hashes. It excludes local Cargo configuration,
credentials and build outputs. A `.sha256` file accompanies it.

The verifier checks hashes and builds with an empty Cargo cache and
`--locked --offline`. Users can unpack the archive and run
`cargo build --release --locked --offline`, without access to crates.io or the
engine repositories. Rust 1.90+ and native GTK/libadwaita/VTE development
packages must already be installed; the archive does not bundle a compiler or
system packages.

Future tagged releases publish the verified archive and checksum alongside
RPM/Flatpak assets. Both package builds consume that same archive. Keep release
assets backed up along with the repository. Existing release assets are not
retroactively changed by this setup. A normal Git checkout still needs upstream
access on its first dependency download; use the release source archive for an
independent build.
