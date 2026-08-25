export prefix ?= /usr
sysconfdir ?= /etc
bindir = $(prefix)/bin
libdir = $(prefix)/lib
sharedir = $(prefix)/share

BINARY = lingmo-comp
CARGO_TARGET_DIR ?= target
TARGET = debug
DEBUG ?= 0

.PHONY = all clean install uninstall vendor

ifeq ($(DEBUG),0)
	TARGET = release
	ARGS += --release
endif

VENDOR ?= 0
ifneq ($(VENDOR),0)
	ARGS += --offline --locked
endif

TARGET_BIN="$(DESTDIR)$(bindir)/$(BINARY)"

KEYBINDINGS_CONF="$(DESTDIR)$(sharedir)/lingmo/com.lingmoos.LingmoSettings.Shortcuts/v1/defaults"
TILING_EXCEPTIONS_CONF="$(DESTDIR)$(sharedir)/lingmo/com.lingmoos.LingmoSettings.WindowRules/v1/tiling_exception_defaults"

all: extract-vendor
	cargo build $(ARGS)

clean:
	cargo clean

distclean:
	rm -rf .cargo vendor vendor.tar target

vendor:
	mkdir -p .cargo
	cargo vendor | head -n -1 > .cargo/config
	echo 'directory = "vendor"' >> .cargo/config
	[ -n "$(SOURCE_GIT_HASH)" ] && printf '\n[env]\nGIT_HASH = "%s"\n' "$(SOURCE_GIT_HASH)" >> .cargo/config || true
	tar pcf vendor.tar vendor
	rm -rf vendor

extract-vendor:
ifeq ($(VENDOR),1)
	rm -rf vendor; tar pxf vendor.tar
endif

install:
	install -Dm0755 "$(CARGO_TARGET_DIR)/$(TARGET)/$(BINARY)" "$(TARGET_BIN)"
	install -Dm0644 "data/keybindings.ron" "$(KEYBINDINGS_CONF)"
	install -Dm0644 "data/tiling-exceptions.ron" "$(TILING_EXCEPTIONS_CONF)"

install-bare-session: install
	install -Dm0644 "data/lingmo.desktop" "$(DESTDIR)$(sharedir)/wayland-sessions/lingmo.desktop"
	install -Dm0644 "data/lingmo-session.target" "$(DESTDIR)$(libdir)/systemd/user/lingmo-session.target"
	install -Dm0644 "data/lingmo-session-pre.target" "$(DESTDIR)$(libdir)/systemd/user/lingmo-session-pre.target"
	install -Dm0644 "data/lingmo-comp.service" "$(DESTDIR)$(libdir)/systemd/user/lingmo-comp.service"
	install -Dm0755 "data/lingmo-service" "$(DESTDIR)/$(bindir)/lingmo-service"

uninstall:
	rm "$(TARGET_BIN)" "$(KEYBINDINGS_CONF)"

uninstall-bare-session:
	rm "$(DESTDIR)$(sharedir)/wayland-sessions/lingmo.desktop"
	rm "$(DESTDIR)$(libdir)/systemd/user/lingmo-session.target"
	rm "$(DESTDIR)$(libdir)/systemd/user/lingmo-session-pre.target"
	rm "$(DESTDIR)$(libdir)/systemd/user/lingmo-comp.service"
	rm "$(DESTDIR)/$(bindir)/lingmo-service"
