# PathPilot v0.3.0

PathPilot v0.3.0 expands the preview from a primarily local file manager into
a practical SFTP and SMB browser while improving long-running file operations.

## Highlights

- Browse, preview, create, rename, copy, move, and delete files through GIO/GVfs
  on SFTP and SMB locations.
- Revisit remote directories quickly with bounded caching, background prefetch,
  and shared in-flight requests across the browser panes.
- Track copy and permanent-delete operations in a non-modal progress shelf with
  aggregate counts, byte progress, current filenames, and cancellation.
- Reuse prepared copy manifests on repeated paste operations instead of scanning
  the same source tree again.
- Browse and edit supported local archives through explicit session save/discard
  handling.
- Save custom bookmarks with user-selected `g` shortcuts.
- Load complete text previews on demand with cancellation and visible loading
  feedback.
- Toggle the command reference with either `F1` or `?`.

## Remote filesystem notes

Remote support depends on host GVfs backends. Fedora users should install
`gvfs` and `gvfs-smb`. Most remote backends do not implement a desktop Trash;
on remote locations, `d d` therefore asks for permanent-delete confirmation.
Remote image previews, archive editing, Git status, the embedded terminal, and
embedded Neovim remain local-only.

## Packages

The GitHub release provides a Fedora RPM, source RPM, standalone Flatpak bundle,
and `SHA256SUMS`. PathPilot remains a preview release; configuration formats may
continue to evolve before 1.0.
