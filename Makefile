SHELL := /usr/bin/env bash

PREFIX ?= $(HOME)/.local
DESTDIR ?=
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
APP_ID := ronnie-3000
BINARY := ronie
VERSION ?= $(shell cargo pkgid ronie-app | cut -d@ -f2)
DIST_DIR ?= dist
ICON_SIZES := 16 32 48 64 128 256 512
VST3DIR ?= $(HOME)/.vst3
VST3_BUNDLE = $(DESTDIR)$(VST3DIR)/Ronnie 3000.vst3
VST3_FOLDER = $(VST3_BUNDLE)/Contents/$(shell uname -m)-linux

.DEFAULT_GOAL := help
.PHONY: help setup doctor build run check package appimage install uninstall vst3 install-vst3 uninstall-vst3

help:
	@printf 'Ronnie 3000\n\n'
	@printf '  make setup           Install Ronnie for this user, with a menu entry and an icon (builds\n'
	@printf '                       from source when Rust is present, otherwise downloads the latest release)\n'
	@printf '  make run             Build and open Ronnie from this checkout\n'
	@printf '  make uninstall       Remove the files that make setup installed\n'
	@printf '  make install-vst3    Build the VST3 plugin and install it in %s (Linux)\n' "$(VST3DIR)"
	@printf '  make uninstall-vst3  Remove the VST3 plugin that make install-vst3 installed\n'
	@printf '  make check           Run the full check: build, clippy, tests, format, and cargo deny\n'
	@printf '  make package         Build a portable Linux tarball in %s/\n' "$(DIST_DIR)"
	@printf '  make appimage        Build a Linux AppImage in %s/\n' "$(DIST_DIR)"

setup:
	@if command -v cargo >/dev/null 2>&1; then \
		$(MAKE) --no-print-directory doctor build install; \
	else \
		printf 'Rust is not installed, so Ronnie downloads the latest release.\n'; \
		PREFIX="$(PREFIX)" sh ./install.sh; \
	fi

doctor:
	@command -v cargo >/dev/null || { printf 'error: cargo is required (https://rustup.rs), or run sh install.sh for a prebuilt release\n' >&2; exit 1; }
	@command -v rustc >/dev/null || { printf 'error: rustc is required\n' >&2; exit 1; }
	@command -v pkg-config >/dev/null || { printf 'error: pkg-config is required\n' >&2; exit 1; }
	@command -v install >/dev/null || { printf 'error: install is required\n' >&2; exit 1; }
	@command -v tar >/dev/null || { printf 'error: tar is required\n' >&2; exit 1; }
	@command -v sha256sum >/dev/null || { printf 'error: sha256sum is required\n' >&2; exit 1; }
	@pkg-config --exists alsa || { printf 'error: ALSA development files are required (libasound2-dev or alsa-lib-devel)\n' >&2; exit 1; }
	@printf 'Build dependencies are available.\n'

build:
	./script/with-env.sh cargo build --release --locked -p ronie-app --bin $(BINARY)

vst3:
	./script/with-env.sh cargo build --release --locked -p ronie-plugin-nice

run:
	./script/with-env.sh cargo run --release --locked -p ronie-app --bin $(BINARY) -- --ui

check:
	./script/check.sh

package: doctor build
	VERSION="$(VERSION)" DIST_DIR="$(DIST_DIR)" bash packaging/linux/package.sh

appimage: doctor build
	VERSION="$(VERSION)" DIST_DIR="$(DIST_DIR)" bash packaging/linux/appimage.sh

install:
	install -Dm755 target/release/$(BINARY) $(DESTDIR)$(BINDIR)/$(BINARY)
	install -Dm644 assets/$(APP_ID).svg $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg
	@for size in $(ICON_SIZES); do \
		install -Dm644 assets/$(APP_ID)-$$size.png $(DESTDIR)$(DATADIR)/icons/hicolor/$${size}x$${size}/apps/$(APP_ID).png; \
	done
	install -Dm644 packaging/$(APP_ID).desktop $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop
	@# The PATH of a desktop session often has no ~/.local/bin. The launcher then hides an entry
	@# whose TryExec it cannot find. A direct install writes the full path of the binary.
	@# A staged install keeps the bare name, because a package puts the binary on the PATH.
	@if [ -z "$(DESTDIR)" ]; then \
		sed -e 's|^Exec=$(BINARY)|Exec="$(BINDIR)/$(BINARY)"|' -e 's|^TryExec=$(BINARY)$$|TryExec=$(BINDIR)/$(BINARY)|' \
			packaging/$(APP_ID).desktop >$(DATADIR)/applications/$(APP_ID).desktop; \
	fi
	install -Dm644 LICENSE $(DESTDIR)$(DATADIR)/doc/ronnie/LICENSE
	@if command -v desktop-file-validate >/dev/null 2>&1; then desktop-file-validate $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop; fi
	@# A staged install (DESTDIR) goes into a package. Its caches would list only Ronnie and
	@# would replace the caches of the system that unpacks the package.
	@if [ -z "$(DESTDIR)" ] && command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database $(DATADIR)/applications; fi
	@# GTK reads the icon folders directly when they have no cache. An old cache hides a new icon,
	@# so refresh only a cache that exists. Icons of other apps can make the refresh fail.
	@if [ -z "$(DESTDIR)" ] && [ -f $(DATADIR)/icons/hicolor/icon-theme.cache ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache -q -t -f $(DATADIR)/icons/hicolor || printf 'warning: the icon cache in %s was not refreshed\n' "$(DATADIR)/icons/hicolor"; \
	fi
	@printf 'Installed %s. Run `%s` or find Ronnie 3000 in your application launcher.\n' "$(DESTDIR)$(BINDIR)/$(BINARY)" "$(BINARY)"

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/$(BINARY)
	rm -f $(DESTDIR)$(DATADIR)/doc/ronnie/LICENSE
	rm -f $(DESTDIR)$(DATADIR)/doc/ronnie/LICENSE-MIT
	rm -f $(DESTDIR)$(DATADIR)/doc/ronnie/LICENSE-APACHE
	rm -f $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop
	rm -f $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/$(APP_ID).svg
	@for size in $(ICON_SIZES); do \
		rm -f $(DESTDIR)$(DATADIR)/icons/hicolor/$${size}x$${size}/apps/$(APP_ID).png; \
	done
	@if [ -z "$(DESTDIR)" ] && command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database $(DATADIR)/applications; fi
	@# GTK reads the icon folders directly when they have no cache. An old cache hides a new icon,
	@# so refresh only a cache that exists. Icons of other apps can make the refresh fail.
	@if [ -z "$(DESTDIR)" ] && [ -f $(DATADIR)/icons/hicolor/icon-theme.cache ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache -q -t -f $(DATADIR)/icons/hicolor || printf 'warning: the icon cache in %s was not refreshed\n' "$(DATADIR)/icons/hicolor"; \
	fi

# The VST3 plugin for Linux. A VST3 bundle is a folder, and on Linux it holds one library.
install-vst3: doctor vst3
	@[ "$$(uname -s)" = Linux ] || { printf 'error: make install-vst3 is for Linux; see the README for macOS and Windows\n' >&2; exit 1; }
	install -Dm755 target/release/libronie_plugin_nice.so "$(VST3_FOLDER)/Ronnie 3000.so"
	@printf 'Installed %s. Scan for new plugins in your DAW, then add Ronnie 3000 to a track.\n' "$(VST3_BUNDLE)"

uninstall-vst3:
	rm -f "$(VST3_FOLDER)/Ronnie 3000.so"
	@for folder in "$(VST3_FOLDER)" "$(VST3_BUNDLE)/Contents" "$(VST3_BUNDLE)"; do \
		if [ -d "$$folder" ]; then rmdir "$$folder" || exit 1; fi; \
	done
