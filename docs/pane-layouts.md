# Pane layouts

The `cycle_layout` command, bound to `z` by default, cycles without changing navigation or selection state:

1. **Browse** shows Parent, Current, and Preview and restores the user's previous divider positions.
2. **Focus preview** hides Parent, moves Current to the left, and gives Preview roughly two thirds of the content width.
3. **Preview only** hides both directory panes so Preview receives the complete content width.

The next invocation returns to Browse. Browse and Focus Preview keep independent divider positions. Window size, maximized state, active layout, divider positions, and the last visited directory are written to `config.toml` when the window closes and restored at the next launch. A missing saved directory safely falls back to the process start directory.

The command is part of `AppCommand`, the TOML keymap, F1 help, Visual-mode allowlist, and command palette. Hidden panes keep their models, cursor, and preview generation alive; cycling changes presentation only.
