# Places and bookmarks

The `g` family provides dynamic navigation continuations:

- `g g` selects the first item in the current directory;
- `g h` opens Home;
- `g d` opens the XDG Downloads directory when it exists;
- `g r` opens filesystem root.

After the first `g`, the bottom interaction panel lists only currently available built-ins plus configured bookmarks. Navigation uses the normal history path, so visited-directory cursor restoration and persisted last location continue to work.

Press `b` to bookmark the current directory. PathPilot first asks for an unused
ASCII letter or digit, then for the bookmark name. Built-in place keys and keys
already assigned to bookmarks are rejected. The new `g` continuation is
available immediately after the name is accepted.

Bookmarks are stored in `bookmarks.toml`. Close PathPilot before editing the
file. Add an inline list:

```toml
bookmarks = [
    { key = "w", label = "Work", uri = "file:///home/roman/work" },
]
```

An empty bookmark list is omitted when PathPilot writes the file. Invalid
configuration is reported in the log and is not overwritten with defaults.
Bookmarks from older `config.toml` files are migrated automatically.

Keys must be one ASCII letter or digit. `g`, `h`, `d`, and `r` are reserved; duplicates, empty labels, and values without a URI scheme reject the settings file and trigger the normal safe fallback. Bookmark locations may use local or future non-local GIO URI schemes.
