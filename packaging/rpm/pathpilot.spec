Name:           pathpilot
Version:        0.1.0
Release:        1%{?dist}
Summary:        Keyboard-first graphical file manager

License:        GPL-3.0-or-later
URL:            https://github.com/RomanRcT/pathpilot
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  desktop-file-utils
BuildRequires:  gcc
BuildRequires:  appstream
BuildRequires:  pkgconfig(gtk4) >= 4.12
BuildRequires:  pkgconfig(vte-2.91-gtk4)
BuildRequires:  rust
Recommends:     neovim

%description
PathPilot is a GTK 4 file manager combining three-column navigation,
Vim-inspired keyboard controls, responsive previews, safe file operations,
desktop application integration, and embedded Neovim editing.

%prep
%autosetup

%build
cargo build --release --locked

%install
%make_install

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/io.github.RomanRcT.PathPilot.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/io.github.RomanRcT.PathPilot.metainfo.xml

%files
%license LICENSE
%doc README.md CHANGELOG.md
%{_bindir}/pathpilot
%{_datadir}/applications/io.github.RomanRcT.PathPilot.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.github.RomanRcT.PathPilot.svg
%{_metainfodir}/io.github.RomanRcT.PathPilot.metainfo.xml

%changelog
* Tue Aug 04 2026 RomanRcT <RomanRcT@users.noreply.github.com> - 0.1.0-1
- First preview package
