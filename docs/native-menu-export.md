# Native menu export over D-Bus

Investigation notes for publishing PathPilot operations to the desktop shell, so the
[Global Menu](https://github.com/Unmade760/orbit-global-menu) GNOME extension can show them in the
top panel. Everything below was measured on the development machine (Fedora Linux 44 Workstation,
GNOME Shell 50.5, mutter 50.5, GTK 4.22.5, Wayland session) rather than read off documentation.

## Question

Global Menu shows the menu of the focused application in the GNOME top panel. Applications reach it
by exporting their menus over the session D-Bus. Should PathPilot export its operations the same
way?

## Verdict

Yes, it is feasible, and the required protocol is still implemented by GTK on this machine. PathPilot
is not exportable yet, because it has no menu model and almost no actions: one in-window
`gtk::MenuButton` holding a single "About PathPilot" item, and one registered action. The native
path is `Gio.Menu` + `Gio.Action`s exported through `Gtk::Application::set_menubar()`; that produces
the `org.gtk.Menus` and `org.gtk.Actions` objects the daemon reads. No third-party code needs to be
added to PathPilot for it.

Two things block an end-to-end claim today:

- the Global Menu companion daemon (`global-menu`) is not installed, only the shell extension;
- window discovery on Wayland is unverified, see [Unverified](#unverified).

## How the pieces fit

Three parties talk on the session bus:

1. **Shell extension** (`global-menu@unmade.space`, v1.0.0, GNOME 49/50). It owns no menu logic of
   its own beyond rendering; on focus change it hands the window's identity to a daemon under bus
   name `space.unmade.GlobalMenu`, object `/space/unmade/GlobalMenu`, interface
   `space.unmade.GlobalMenu`. Methods observed in its code: `WindowSwitched(a{ss})`,
   `RequestMenuTree()`, `ActivateMenuItem(s)`, `ListWindowActions(as)`; the daemon answers with
   `SendMenuTree(s)` or the fallback/window-action signals. It tries, in priority order, the native
   application menu, then a curated per-application keyboard-shortcut menu, then a generic window
   action menu.
2. **Companion daemon** (`global-menu`, upstream source inside
   [Fildem/gnomehud](https://github.com/Fildem/gnomehud)). Python plus `dbus-python`. It reads the
   menu tree from the application, and activates items by calling into the application. It expects
   per-window properties `xid`, `gtk_unique_bus_name`, `gtk_application_id`,
   `gtk_application_object_path`, `gtk_window_object_path`, `gtk_app_menu_object_path`,
   `gtk_menubar_object_path`. Because Bamf is X11-only, the extension supplies these on Wayland.
3. **Application** (us). It must export the menu tree and activate actions when asked.

The daemon knows two native menu sources:

- **GTK source**: read the `_GTK_UNIQUE_BUS_NAME`, `_GTK_APPLICATION_OBJECT_PATH`,
  `_GTK_WINDOW_OBJECT_PATH`, `_GTK_MENUBAR_OBJECT_PATH`, `_GTK_APP_MENU_OBJECT_PATH` hints, then use
  `org.gtk.Menus` to fetch the tree and `org.gtk.Actions.Activate(name, [], {})` to fire an item.
- **Canonical source**: `com.canonical.AppMenu.Registrar` plus `com.canonical.dbusmenu`, keyed by an
  X11 XID. Available on this machine only as ayatana GObject introspection typelibs
  (`Dbusmenu-0.4`, `DbusmenuGtk3-0.4`); it needs an X11 window id, so it is dead on this Wayland
  session and is not a design option.

## Verified

### The exporter is GIO, not GTK widget code

The interface descriptions for `org.gtk.Menus`, `org.gtk.Actions`, and `org.gtk.Application` are
found in `/usr/lib64/libgio-2.0.so.0`, and not in `libgtk-4.so.1` or `libgtk-3.so.0`. There is no
`g_menu_exporter_*` symbol in any installed library. The export is wired by `GtkApplication`, so a
plain `Gio.Application` never exports menus on its own.

### A minimal GTK 4 application exports everything the daemon needs

Probe: `/tmp/menubar_probe.py` (PyGObject, `Gtk 4.0`) registers application id
`io.test.MenubarProbe`, adds one stateless and one stateful action, builds a `Gio.Menu` menubar with
a `_File` submenu, and calls `set_menubar()`. Observed on the session bus:

| Object path | Interfaces |
| --- | --- |
| `/io/test/MenubarProbe` | `org.gtk.Actions`, `org.gtk.Application`, `org.freedesktop.Application` |
| `/io/test/MenubarProbe/window/1` | `org.gtk.Actions` |
| `/io/test/MenubarProbe/menus/menubar` | `org.gtk.Menus` |

Menu contents are readable from another process:

```console
$ busctl --user call io.test.MenubarProbe /io/test/MenubarProbe/menus/menubar \
    org.gtk.Menus Start au 1 0
a(uuaa{sv}) 1 0 0 1 2 "label" s "_File" ":submenu" (uu) 1 0
$ busctl --user call io.test.MenubarProbe /io/test/MenubarProbe org.gtk.Actions Describe s about-probe
(bgav) true "" 0
$ gdbus call --session --dest io.test.MenubarProbe --object-path /io/test/MenubarProbe \
    --method org.gtk.Actions.Activate about-probe "[]" "{}"
()          # and the application printed "ACTION FIRED"
```

So the whole data path the daemon uses — read tree, read enabled/state, activate — works against a
GTK 4.22 application.

### The compositor side exists

`libmutter-mtk-13.so` carries `gtk_unique_bus_name`, `gtk_application_object_path`,
`gtk_window_object_path`, `gtk_menubar_object_path`, `gtk_app_menu_object_path`, `gtk_shell`, and
`gtk-shell-surface-data`. GTK 4.22 carries `gtk-shell-shows-menubar` and
`gtk-shell-shows-app-menu`. These are the strings the extension reads through `MetaWindow`, so the
plumbing is present in the running version.

### The extension is loaded, the daemon is not

The extension is in `enabled-extensions` and runs in the current session; its own warning shows in
the journal:

```
gnome-shell[...]: [global-menu] global-menu is not on PATH. Menus exported by
applications need it; install the global-menu package.
```

`which global-menu` finds nothing. So the native source can never resolve on this machine until the
daemon is installed.

### Rust bindings are already sufficient

The pinned `gtk4` crate (0.10.3, workspace feature `v4_12`) exposes `ApplicationExt::set_menubar()`
and the `menubar()` builder, ungated behind any feature flag, and `GtkApplication` has a writable
`menubar` property. No extra crate or system dependency is needed to build a PathPilot menubar.

## Unverified

Whether mutter reports the `_GTK_*` menu hints for a GTK 4 window on Wayland. To find out without the
real daemon, a temporary process claimed `space.unmade.GlobalMenu` and logged the `WindowSwitched`
payload. It logged a direct `busctl` call, proving the receiver works, but no call ever arrived for a
probe window: the probe window never received focus, and no GUI automation is available on this
machine (`xdotool`, `ydotool`, `wtype` absent). Confirming discovery therefore needs one real focus
change while a fake or real daemon listens, which means a human at the desktop, or a new session.

Note from the extension README: on Wayland, a session restart is needed after enabling it, so any
test should run after a fresh login.

## Implementation plan, if we proceed

1. **Action layer.** Register `Gio::SimpleAction`s: application-wide ones (`about`, `quit`,
   `cycle-layout`) and window-scoped ones for every `AppCommand` variant (22 today), with `enabled`
   and `state` reflecting real preconditions (paste needs a clipboard, rename and trash need a
   selection). Each handler dispatches to the existing implementation.
2. **Menu model.** Build a `Gio::Menu` (File / Edit / View / Go / Help) from the palette metadata
   that already exists in `pathpilot_core::PALETTE_COMMANDS`, and call `set_menubar()`.
3. **Single source of truth.** One command, label, and shortcut table in `pathpilot_core`, so the
   palette, the keymap, the TOML key bindings, and the menu cannot drift apart.
4. **Tests.** Unit tests mapping `AppCommand` to menu item, plus a manual checklist: item appears in
   the panel, activating it drives the same code path as the palette, disabled items are greyed out.
5. **Version floor.** `set_menubar` exists since GTK 4.20 and the symbol is resolved at load time,
   so either raise the documented minimum from GTK 4.12 or set the property dynamically.

## Risks

- **Discovery is the whole risk.** If mutter does not propagate the hints on Wayland, nothing appears
  in the panel and the alternative protocol needs an X11 window id. Verify before writing code.
- **Unshipped dependency.** `global-menu` is not packaged for Fedora (AUR only), runs as an on-demand
  systemd user service, and needs a fresh session on Wayland. A feature that needs a third-party
  daemon to be installed will read as "broken" to most users.
- **Surface drift.** 22 commands times four surfaces (palette, keymap, TOML config, menu).
- **Minimum GTK bump** touches `README.md`, `docs/packaging.md`, CI, and the Flatpak manifest.
- **Session bus exposure.** Any session-bus client can activate exported actions; keep them to the
  same operations the palette already offers.
- **Multiple windows.** The daemon picks the focused window's object path, so window-scoped actions
  must be registered per window.

## Reproducing the probe

```console
$ WAYLAND_DISPLAY=wayland-0 python3 menubar_probe.py 4.0 &
$ busctl --user tree io.test.MenubarProbe   # shows /menus/menubar and /window/1
```

The probe is throwaway scratch and is not part of the repository.
