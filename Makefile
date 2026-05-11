.PHONY: client-dist server-build server-check server-musl all clean

all: client-dist server-build

client-dist:
	$(MAKE) -C client dist

server-build:
	cd server && cargo build

server-check:
	cd server && cargo fmt && cargo check

server-musl:
	cd server && ./scripts/build-musl.sh

clean:
	$(MAKE) -C client clean
	rm -rf server/dist server/target
