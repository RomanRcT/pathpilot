# Phase 0 performance spike

## Scope

The spike renders 100,000 synthetic file names in a `GtkListView` and supports `j`, `k`, `gg`, and `G` navigation. It records model creation and window presentation durations through `tracing`.

## Instrumentation

Run the application with:

```bash
RUST_LOG=pathpilot=debug cargo run -p pathpilot
```

Relevant log fields include:

- `model_creation_ms`: creation of the 100,000 strings and `GtkStringList`;
- `elapsed_ms` on `window presented`: application activation through presentation;
- `selected_index`: each command-driven selection change.

## Implementation observations

`GtkListView` is backed by a list model and a `SignalListItemFactory`; it does not allocate a permanent GTK row widget for every entry. The model intentionally contains all synthetic strings so this spike measures GTK model population as well as virtualized row presentation.

## Local measurements

Measured on the developer Fedora workstation on 2026-08-03 using an unoptimized debug build:

| Measurement | Result |
|---|---:|
| Synthetic model creation (100,000 entries) | 126 ms |
| GTK activation through window presentation | 289 ms |

The application window opened successfully and remained responsive during the launch check. Interactive latency for repeated `j`/`k`, `gg`, and `G`, plus row-recycling confirmation in GTK Inspector, still require a hands-on visual check before Phase 1.

## Exit criteria

- No visible freeze while navigating the synthetic list.
- `G` and `gg` scroll immediately to the last and first entry.
- GTK Inspector confirms that row widgets are recycled instead of growing with the model.
- Selection changes update the status line within one frame under normal load.
