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

.DEFAULT_GOAL := help
.PHONY: help setup doctor build run check package appimage install uninstall

help:
	@printf 'Ronnie 3000\n\n'
	@printf '  make setup      Install Ronnie for this user (builds from source when Rust is present,\n'
	@printf '                  otherwise downloads the latest release)\n'
	@printf '  make run        Build and open Ronnie from this checkout\n'
	@printf '  make uninstall  Remove the files that make setup installed\n'
	@printf '  make check      Run the full check: build, clippy, tests, format, and cargo deny\n'
	@printf '  make package    Build a portable Linux tarball in %s/\n' "$(DIST_DIR)"
	@printf '  make appimage   Build a Linux AppImage in %s/\n' "$(DIST_DIR)"

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
	install -Dm644 LICENSE $(DESTDIR)$(DATADIR)/doc/ronnie/LICENSE
	@if command -v desktop-file-validate >/dev/null 2>&1; then desktop-file-validate $(DESTDIR)$(DATADIR)/applications/$(APP_ID).desktop; fi
	@# A staged install (DESTDIR) goes into a package. Its caches would list only Ronnie and
	@# would replace the caches of the system that unpacks the package.
	@if [ -z "$(DESTDIR)" ] && command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database $(DATADIR)/applications; fi
	@if [ -z "$(DESTDIR)" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f $(DATADIR)/icons/hicolor; fi
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
	@if [ -z "$(DESTDIR)" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f $(DATADIR)/icons/hicolor; fi
