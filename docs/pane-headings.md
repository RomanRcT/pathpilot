# Pane location headings

Pane headings combine a stable role (`Parent` or `Current`) with a compact presentation location. Local `file://` URIs are percent-decoded and displayed as filesystem paths, with the user's Home prefix abbreviated to `~`. Filesystem root remains `/` and non-local backends retain their URI scheme.

Long headings use middle ellipsizing. The full decoded local path, or full backend URI, remains available as a tooltip. The top location label follows the same formatting rules while its tooltip preserves the exact internal URI.

Formatting lives in `pathpilot-core`; backend-neutral `Location` values and navigation behavior are unchanged.
