# PathPilot v0.4.0

PathPilot v0.4.0 expands the keyboard-first workflow with tabs and independent
viewer windows, native document previews and styled Markdown. This remains a
preview release; configuration and behavior may evolve before 1.0.

## Reading without losing your place

- PDF pages render through Poppler. DOCX uses WordCraft and XLSX uses GridCraft
  directly, avoiding LibreOffice conversion in the common case. Other office
  formats and unsupported spreadsheet visuals use the LibreOffice fallback.
- Document previews cache sessions and pages, stay centered on resize and use
  `n` / `N` for next / previous page.
- Markdown uses pulldown-cmark and WebKitGTK with Compact (default), GitHub and
  Reading styles, local relative images, heading anchors and system link opening.
  Raw HTML and page scripts are disabled; remote images are not loaded.
- Unknown MIME types are checked for readable UTF-8 text. Markdown above 24 KiB
  keeps HTML rendering; automatic text preview is capped at 256 KiB with `u`
  available to request the full document.
- `o v` opens a separate viewer at the current document page. It has independent
  file/page navigation and survives closing the source browser or tab. Use
  `j` / `k` for neighboring files, `n` / `N` for pages and `q` to close it.

## A smoother keyboard workflow

- `t c`, `t x`, `t 1`…`t 9` create, close and select tabs. `t s` selects with
  `j` / `k`; a 500 ms idle delay accepts the selection, or set
  `[ui] tab_switch_delay_ms = 0` to require Enter. It does nothing with one tab.
- Tabs keep independent browser state while sharing the file-operation clipboard.
- Deletion and Keep Both confirmations use inline `y` / `n` input. Set
  `[ui] keyboard_confirmations = false` to restore traditional dialogs.
- Filename find and filtering support optional idle commit (800 ms by default).
- The parent pane shows only names. Other lists prioritize names and omit the
  redundant Folder/File column as metadata yields space in narrow panes.
- `?` / F1 and the main menu's Keyboard Shortcuts show compact, theme-colored
  groups. File changes and clipboard/selection are separate; `g` / `b` distinguish
  local places, network connections and bookmarks.
- About works after the original application window has closed.

## Installation and builds

Download the Fedora 44 x86_64 RPM, Ubuntu 24.04 amd64 DEB or x86_64 Flatpak
bundle from this release. Install the DEB with `sudo apt install ./pathpilot_0.4.0-1_amd64.deb`.
Older Ubuntu/Debian distributions with GTK below 4.12 require Flatpak.
`SHA256SUMS` covers the attached assets. The `offline-source` archive includes
locked Rust dependencies, Craft compile-time assets and license notices, and is
built in CI with an empty Cargo cache and network access disabled for Cargo.
GitHub's automatic source archives do not include vendored dependencies.

WebKitGTK 6.0 is now a required native dependency; RPM dependencies and the
GNOME 50 Flatpak runtime provide it. PDF preview requires Poppler tools; legacy
Office conversion requires LibreOffice. Direct DOCX/XLSX rendering needs neither
LibreOffice nor a network connection in the installed application.

WordCraft/GridCraft revisions are pinned. Renovate proposes updates for review;
automatic merging is disabled. About and installed notices credit the document
engines, bundled fonts and GitHub Markdown CSS.

## Known limitations

Tabs are not restored after restarting. Documents are read-only, limited to
64 MiB, and password-protected files are unsupported. Office pagination depends
on installed fonts. Spreadsheet formulas use saved results; unsupported charts,
images and formatting trigger the compatibility fallback. Unknown text detection
supports UTF-8. Remote relative Markdown images are not shown. Drag-and-drop,
audio/video preview and the broader accessibility review remain future work.
