# Remote filesystems

Press `Ctrl+L` and enter a GIO URI to open a remote location. The first
supported backends are SFTP and SMB:

```text
sftp://user@example.com/home/user
smb://server/share/folder
```

PathPilot asks GIO to mount the enclosing volume. GTK presents the native GVfs
authentication dialog when credentials or host confirmation are required.
Passwords should be entered in that dialog rather than embedded in a URI.
Previously mounted locations open without another prompt. The last remote URI
is reconnected when PathPilot starts again.

Completed remote directory listings are kept in a bounded in-memory cache.
The preview pane prefetches the selected directory, navigation reuses an
in-flight prefetch, and the parent pane displays a cached parent immediately
without starting another network request. Press `Ctrl+R` to explicitly reload
the current remote directory.

Directory browsing, text preview, bookmarks, create, rename, copy, move, and
delete use the same GIO operations as local files. Actual capabilities still
depend on the server and backend. For example, a server may reject writes or
the Trash operation even though browsing succeeds. Errors are shown in the
status line and operations remain cancellable.

Features that launch local processes or require native paths remain disabled
for remote locations: the embedded terminal, Neovim, Git status, archive
editing, and path-based image decoding.

The host must provide the corresponding GVfs backend. On Fedora, install the
base package for SFTP and the SMB backend with:

```sh
sudo dnf install gvfs gvfs-smb
```
