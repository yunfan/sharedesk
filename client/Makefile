DIST_DIR := dist
SRC_DIR := src

.PHONY: clean build dist

clean:
	rm -rf $(DIST_DIR)

build:
	mkdir -p $(DIST_DIR)
	cp $(SRC_DIR)/index.html $(DIST_DIR)/index.html
	cp $(SRC_DIR)/app.js $(DIST_DIR)/app.js
	cp $(SRC_DIR)/style.css $(DIST_DIR)/style.css
	printf 'window.__ROOM_CONFIG__ = { VITE_BACKEND_URL: "%s" };\n' "$${VITE_BACKEND_URL:-/backend}" > $(DIST_DIR)/config.js

dist: build
