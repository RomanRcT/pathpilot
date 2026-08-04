# Phase 4: configurable keymap

PathPilot loads keyboard bindings from `$XDG_CONFIG_HOME/pathpilot/keymap.toml`, or `~/.config/pathpilot/keymap.toml` when `XDG_CONFIG_HOME` is unset. If the file does not exist, the embedded [`data/default-keymap.toml`](../data/default-keymap.toml) is used.

The `[bindings]` table maps stable command names to a one- or two-character sequence. A command may also use an array to expose multiple bindings. The current file is a complete keymap rather than a partial override.

```toml
[bindings]
navigate_down = ["j", "s"]
navigate_up = ["k", "w"]
create_file = "af"
```

Unknown commands, empty binding arrays, sequences longer than two characters, duplicate sequences, unreadable files, and malformed TOML reject the complete user file. PathPilot logs a structured warning and safely falls back to the embedded defaults.

Supported command names are `navigate_up`, `navigate_down`, `open`, `parent`, `first`, `last`, `quit`, `create_file`, `create_directory`, `rename`, `trash`, `delete_permanently`, `copy`, `cut`, `paste`, and `visual`. Conventional GTK shortcuts and modal controls such as `Escape`, `F1`, Find, and command-palette activation remain fixed in this slice.
