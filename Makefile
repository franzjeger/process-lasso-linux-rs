PREFIX    ?= $(HOME)/.local
BINDIR     = $(PREFIX)/bin
ICONDIR    = $(PREFIX)/share/icons/hicolor/64x64/apps
DESKTOPDIR = $(PREFIX)/share/applications
SYSTEMDDIR = $(HOME)/.config/systemd/user

BINARY     = target/release/process-lasso

.PHONY: build install uninstall enable disable

build:
	cargo build --release

install: build
	@echo "Installing binary…"
	install -Dm755 $(BINARY) $(BINDIR)/process-lasso
	@echo "Installing icon…"
	install -Dm644 assets/icon.png $(ICONDIR)/process-lasso-linux.png
	@echo "Installing .desktop entry…"
	install -Dm644 dist/process-lasso.desktop $(DESKTOPDIR)/process-lasso.desktop
	@echo "Installing systemd user service…"
	install -Dm644 dist/process-lasso.service $(SYSTEMDDIR)/process-lasso.service
	systemctl --user daemon-reload
	@echo "Done. Run 'make enable' to autostart on login."

uninstall:
	rm -f $(BINDIR)/process-lasso
	rm -f $(ICONDIR)/process-lasso-linux.png
	rm -f $(DESKTOPDIR)/process-lasso.desktop
	systemctl --user disable --now process-lasso.service 2>/dev/null || true
	rm -f $(SYSTEMDDIR)/process-lasso.service
	systemctl --user daemon-reload
	@echo "Uninstalled."

enable:
	systemctl --user enable --now process-lasso.service
	@echo "process-lasso will start automatically on login."

disable:
	systemctl --user disable --now process-lasso.service
	@echo "Autostart disabled."
