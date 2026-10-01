# Name filter mode

`/` filters the current directory by name. Unlike filename find, the filter
removes rows from the pane model, so only matching entries stay visible.

## Workflow

Press `/`, type a few characters, then keep navigating. The rows narrow on every
keystroke, and about a second later the browser treats the query as finished: it
applies the filter, closes the input box, and `j`, `k`, `h`, `l`, `g` and `G`
belong to navigation again without pressing `Enter` or `Escape`.

## Key bindings

- `/` starts filtering and shows every entry again.
- Typing narrows the list on every keystroke, case-insensitively, against the
  display name.
- `Backspace` edits the query.
- `Enter` applies the filter and returns to Normal mode with the reduced set in
  place.
- `Escape` clears the filter, both while the query is being typed and afterwards
  from Normal mode, which brings every row back.
- `Ctrl+R` reloads the directory with the applied filter still in force.

## Commit delay

After the last keystroke the browser waits `[ui] query_commit_delay_ms`
(default 800 ms) and then accepts the query on its own: the status returns to
`NORMAL  Filter "…" applied · N shown`, the input box disappears, and the
navigation keys work again. The applied filter stays on the pane, so scrolling a
shortened list keeps preview and selection working.

The delay restarts on every character typed, including `Backspace`, and it only
runs once the query is non-empty, so pressing `/` and thinking is not
interrupted. `query_commit_delay_ms = 0` turns automatic acceptance off and
leaves `Enter` and `Escape` as the only ways out.

Because the delay hands the keys back to the command parser, a query you are
still thinking about can be accepted too early, and the next characters would
then run commands (`l` opens, `e` opens in Neovim). Raise the delay if you type
slowly. To continue editing after acceptance, press `/` again: the query starts
empty and every row comes back with it.

## Scope

Filtering covers the current column only. The parent and directory-preview
columns keep their full listings, and navigating to another directory clears the
filter, matching the behaviour of filename find. Hidden entries never appear just
because they match: the hidden-items setting is applied first.

## Implementation

`AppMode::Filter` owns the input and is mutually exclusive with Find, Command,
Text Input, and Visual. `DirectoryPane` retains the full listing of the current
load and rebuilds its `GtkListStore` rows from that list on each keystroke
through `apply_filter()`. The cursor returns to the previously selected entry
when it still matches and otherwise moves to the first match, so row positions
stay valid while the model shrinks. Automatic acceptance runs on a `glib` timeout
armed by the keyboard controller; the timeout clears its own source id when it
fires, and the key handler cancels it on `Enter` and `Escape`.

## Verification

Core tests cover the explicit Filter transitions and the case-insensitive
substring match. There is no GTK driver in this repository, so filtering needs a
short manual check. Run the workspace binary (`cargo run`, or
`./target/release/pathpilot` after `cargo build --release`), not a `pathpilot`
found on `PATH`, which may be an old installed copy.

`/` and typing hides non-matching rows immediately, stopping typing applies the
filter and closes the box on its own, `j`/`k` then move through the reduced set
without any extra key, `Enter` accepts early, `Escape` restores every row both
while typing and after acceptance, and navigating to another directory brings
every row back.
