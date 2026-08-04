# Mode-aware status line

The status line is the compact state and context surface; command discovery remains in the separate interaction panel. Its full background changes by application mode while the textual `NORMAL`, `VISUAL`, `FIND`, `COMMAND`, or `INPUT` name remains visible for accessibility.

In Normal mode the selected item contributes fixed-position columns for Unix permissions, local modified timestamp, and size, followed by the variable-width content type. Visual mode keeps the selection count and the active item's metadata. A Nerd Font Mono stack with a standard monospace fallback prevents columns from shifting as selection changes. Operation progress, failures, and clipboard state continue to use the same line without navigation-help duplication.

For local directories, a separate right-aligned Git block uses Nerd Font symbols for branch and clean/modified state. A background `git status` probe supplies it; probes carry a navigation generation and stale results are discarded. Non-repository directories simply omit the block, and no Git process runs on the GTK thread.
