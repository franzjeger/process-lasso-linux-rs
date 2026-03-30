PREFIX    ?= $(HOME)/.local
BINDIR     = $(PREFIX)/bin
ICONBASE   = $(PREFIX)/share/icons/hicolor
DESKTOPDIR = $(PREFIX)/share/applications
SYSTEMDDIR = $(HOME)/.config/systemd/user

BINARY     = target/release/argus-lasso

.PHONY: build install reinstall uninstall enable disable install-icons

build:
	cargo build --release

install-icons:
	@echo "Installing icon at all standard sizes…"
	@for size in 16 22 24 32 48 64 128 256; do \
		dir=$(ICONBASE)/$${size}x$${size}/apps; \
		mkdir -p "$$dir"; \
		magick -resize $${size}x$${size} assets/icon.png "$$dir/argus-lasso.png" 2>/dev/null \
		  || convert -resize $${size}x$${size} assets/icon.png "$$dir/argus-lasso.png" 2>/dev/null \
		  || cp assets/icon.png "$$dir/argus-lasso.png"; \
	done

install: build install-icons
	@echo "Installing binary…"
	install -Dm755 $(BINARY) $(BINDIR)/argus-lasso
	@echo "Installing .desktop entry…"
	sed 's|^Exec=argus-lasso|Exec=$(BINDIR)/argus-lasso|' dist/argus-lasso.desktop > $(DESKTOPDIR)/argus-lasso.desktop
	chmod 644 $(DESKTOPDIR)/argus-lasso.desktop
	@echo "Installing systemd user service…"
	install -Dm644 dist/argus-lasso.service $(SYSTEMDDIR)/argus-lasso.service
	systemctl --user daemon-reload
	@echo "Refreshing icon and desktop caches…"
	-update-desktop-database $(DESKTOPDIR)
	-gtk-update-icon-cache -f -t $(PREFIX)/share/icons/hicolor/
	-kbuildsycoca6 --noincremental 2>/dev/null || kbuildsycoca5 --noincremental 2>/dev/null || true
	@echo "Done. Run 'make enable' to autostart on login."

reinstall: build
	@echo "Installing binary…"
	install -Dm755 $(BINARY) $(BINDIR)/argus-lasso
	@echo "Restarting argus-lasso…"
	@if systemctl --user is-active --quiet argus-lasso.service; then \
		systemctl --user restart argus-lasso.service; \
		echo "Restarted via systemd."; \
	else \
		pkill -x argus-lasso 2>/dev/null || true; \
		nohup $(BINDIR)/argus-lasso &>/dev/null & \
		echo "Restarted as background process."; \
	fi

uninstall:
	rm -f $(BINDIR)/argus-lasso
	find $(ICONBASE) -name "argus-lasso.png" -delete 2>/dev/null || true
	rm -f $(DESKTOPDIR)/argus-lasso.desktop
	systemctl --user disable --now argus-lasso.service 2>/dev/null || true
	rm -f $(SYSTEMDDIR)/argus-lasso.service
	systemctl --user daemon-reload
	@echo "Uninstalled."

enable:
	systemctl --user enable --now argus-lasso.service
	@echo "argus-lasso will start automatically on login."

disable:
	systemctl --user disable --now argus-lasso.service
	@echo "Autostart disabled."
