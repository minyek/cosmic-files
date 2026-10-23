ExcludeArch: %{ix86}

Name:           cosmic-files
Version:        %{rpm_version}
Release:        1%{?dist}.local
Summary:        Libcosmic file manager
License:        GPL-3.0-only
URL:            https://github.com/pop-os/cosmic-files
Source0:        cosmic-files-source.tar.gz

BuildRequires:  cargo
BuildRequires:  desktop-file-utils
BuildRequires:  glib2-devel
BuildRequires:  just
BuildRequires:  lld
BuildRequires:  libxkbcommon-devel
BuildRequires:  rust

Requires:       gvfs
Requires:       gvfs-fuse
Requires:       xdg-utils
Requires:       cosmic-icon-theme >= %{rpm_base_version}
Recommends:     gvfs-nfs
Recommends:     gvfs-smb

%description
Libcosmic file manager.

%prep
%setup -q -n cosmic-files

%build
export VERGEN_GIT_SHA="%{source_commit}"
export VERGEN_GIT_COMMIT_DATE="%{source_commit_date}"
export RUSTFLAGS="${RUSTFLAGS:+${RUSTFLAGS} }--cfg=io_uring_skip_arch_check"
just build-release --locked

%install
just rootdir=%{buildroot} prefix=%{_prefix} install

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/com.system76.CosmicFiles.desktop

%files
%license LICENSE
%doc README.md
%{_bindir}/cosmic-files
%{_bindir}/cosmic-files-applet
%{_bindir}/cosmic-files-thumbnailer
%{_datadir}/applications/com.system76.CosmicFiles.desktop
%{_datadir}/thumbnailers/com.system76.CosmicFiles.thumbnailer
%{_metainfodir}/com.system76.CosmicFiles.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/com.system76.CosmicFiles.svg
