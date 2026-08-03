# Milestone 5: preview prototype

Milestone 5 replaces the directory-only third column with a coordinated preview stack.

## Request lifecycle

1. A selection change immediately displays filename, MIME type, size, and URI.
2. The previous debounce source, GIO request, image decode, and directory enumeration are canceled.
3. The preview generation advances, invalidating any result already racing with cancellation.
4. After 75 ms, only the latest request starts.
5. The result is rendered only if its generation remains current.

## Providers

### Text

Text and common source-code MIME types are read through `GInputStream::read_bytes_async`. The loader requests at most 1 MiB plus one byte, uses the extra byte to report truncation, rejects NUL-containing binary data, and converts malformed UTF-8 lossily instead of failing the application.

Zero-byte files bypass MIME dispatch and render an explicit empty-file state. YAML is recognized through `text/yaml`, `application/yaml`, and the legacy `application/x-yaml` MIME type.

### Images

Image streams are opened asynchronously and decoded with `GdkPixbuf`'s asynchronous scaled-stream API. Decoding is bounded to a 1200 × 1200 preview while preserving aspect ratio. The resulting pixbuf is converted to a GDK texture for `GtkPicture`.

### Directories and unsupported files

Directories retain the independently cancelable `DirectoryPane`. Unsupported types remain on the metadata view with a clear status instead of producing an error dialog.

## Verification

- `PreviewGate` tests prove stale generations are rejected.
- An async integration test reads a six-byte text file with a four-byte limit and verifies both the visible content and truncation flag.
- Existing filesystem and navigation tests continue to pass under the expanded workspace.

Runtime verification on the developer Fedora workstation on 2026-08-03 presented the debug window successfully, loaded the 14-entry current and 98-entry parent models, then started the selected directory preview after the debounce interval. The preview completed without stale updates or runtime errors.
