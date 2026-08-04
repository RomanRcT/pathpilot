# Open With

Press `o` on a regular file to open the MIME-aware application panel. Saved
applications are shown as numbered continuations (`o 1` through `o 9`), and
`o e` opens the system GTK/GNOME application chooser.

Choosing an application from the system dialog launches the selected file and
appends the application's desktop ID to PathPilot's history for that content
type. Repeated choices are deduplicated. PathPilot does not change the desktop
default itself; any default-application choice remains under the system
dialog's control.

History is persisted in `config.toml`, for example:

```toml
[open_with]
"application/xml" = ["org.gnome.TextEditor.desktop"]
```

Open With is not offered for directories. Missing applications are skipped
when the numbered list is built, unknown content types fail safely, and launch
errors identify both the file and application in the status line and log.
