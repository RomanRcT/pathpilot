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

Supported command names are `navigate_up`, `navigate_down`, `open`, `parent`, `first`, `last`, `quit`, `create_file`, `create_directory`, `rename`, `trash`, `delete_permanently`, `copy`, `cut`, `paste`, `visual`, and `cycle_layout`. F1/`?` help, pending-sequence hints, and command-palette shortcut search use the validated runtime bindings. Conventional GTK shortcuts and modal controls such as `Escape`, `F1`, `?`, Find, and command-palette activation remain fixed.

PathPilot also loads `config.toml` from the same directory. UI defaults are filled for missing fields, malformed settings fall back safely, and the F1/`?` help preference is persisted immediately. Current conventional shortcuts are `Ctrl+C`, `Ctrl+X`, `Ctrl+V`, `Ctrl+N`, `Ctrl+Shift+N`, `Ctrl+Q`, `F2`, `Delete`, and `Shift+Delete`.

Tab controls are fixed modal sequences: `t c` creates a tab, `t x` closes it,
`t 1`…`t 9` selects by number, and `t s` enters selection with `j`/`k`.
Enter accepts and Escape restores the original tab. The `[ui]`
`tab_switch_delay_ms` setting defaults to 500 ms after a selection key;
0 disables automatic acceptance. The `t` prefix is reserved by the UI, like
`s` (sorting) and `o` (Open With).

`o v` opens an independent read-only viewer for the selected file. In that
window, `j`/`k` navigates sibling files, `n`/`N` changes document pages, `u` loads
a full preview, Ctrl+R refreshes, and `q` closes only the viewer.

The `?` / `F1` hint panel groups commands by navigation, search/sorting, files, clipboard/selection, preview, tabs, places, and interface controls. The main menu’s **Keyboard Shortcuts** opens the same grouped reference in a scrollable dialog. The `g` and `b` panels separate local navigation, network connections, and user bookmarks; `b` also explains how to choose an unused bookmark key.
