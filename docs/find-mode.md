# Filename find mode

Filename find provides fast keyboard navigation within the current directory without changing or filtering its model.

## Key bindings

- `f` starts an incremental, case-insensitive filename search from the current selection.
- Typing updates the query and jumps to the nearest matching entry, wrapping at the end of the directory.
- `Enter` accepts the selected match.
- `Escape` cancels the search and restores the original selection.
- `Backspace` edits the query.
- `n` and `N` repeat the accepted search forward and backward.

The query and no-match feedback appear in the foreground overlay. Stopping typing
for `[ui] query_commit_delay_ms` (default 800 ms, `0` to disable) accepts the
search the same way `Enter` does, so `n` and `N` work straight after it and the
navigation keys are usable again. Navigating to another directory clears the
saved query.

## Deliberate separation from filtering

Find mode keeps every directory entry visible and only moves the selection. The separate `/` binding instead removes non-matching rows from the model; see [filter-mode.md](filter-mode.md).
