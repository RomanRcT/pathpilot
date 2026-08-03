# Milestone 3: real directory model

Milestone 3 replaces the Phase 0 synthetic list with asynchronous enumeration of the process working directory.

## Pipeline

1. The UI advances its directory generation and cancels the previous `gio::Cancellable`.
2. `pathpilot-fs-local` requests a minimal GIO attribute set.
3. `GFileEnumerator` returns up to 256 entries per asynchronous batch.
4. Each batch is converted into GTK-independent `FileEntry` values.
5. The UI rejects events whose generation is no longer current.
6. Accepted entries are appended to `GListStore` and sorted through `GtkSortListModel`.

The GTK main thread receives completed batches and performs only model updates. Directory enumeration itself uses GIO's asynchronous API.

## Attributes

- name and display name;
- file type;
- hidden and symbolic-link flags;
- size;
- modification time;
- content type.

The content type supplies a native GIO icon in the row factory. Rows also display the entry kind, formatted size, and local modification time.

## Verification

- Core tests cover URI locations, directory-first name sorting, and stale generation rejection.
- The local-filesystem integration test enumerates a temporary directory through the asynchronous GIO pipeline and validates typed metadata.
- `cargo fmt`, `cargo test`, and strict workspace Clippy are required before completion.

Runtime verification on the developer Fedora workstation on 2026-08-03 loaded the 13-entry project directory in 109 ms. The application window was presented in 104 ms using an unoptimized debug build.
