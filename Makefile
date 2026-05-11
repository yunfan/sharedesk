.PHONY: client-dist server-build server-check server-musl turn-build turn-check turn-musl all clean

all: client-dist server-build

client-dist:
	$(MAKE) -C client dist

server-build:
	cd server && cargo build

server-check:
	cd server && cargo fmt && cargo check

server-musl:
	cd server && ./scripts/build-musl.sh

turn-build:
	cd turn && cargo build

turn-check:
	cd turn && cargo check

turn-musl:
	cd turn && ./scripts/build-musl.sh

clean:
	$(MAKE) -C client clean
	rm -rf server/dist server/target turn/dist turn/target
