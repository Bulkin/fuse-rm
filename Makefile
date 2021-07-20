test:
	fusermount -u test-data/target
	cargo run -- test-data/source test-data/target
	ls test-data/target

toltec:
	podman run --name remarkable-build -v deps:/root/.cargo/registry -v .:/project --rm -it  ghcr.io/toltec-dev/rust:v2.1 make -C /project toltec-internal

toltec-internal:
	opkg update
	opkg install libfuse3
	ln -sf /opt/x-tools/arm-remarkable-linux-gnueabihf/arm-remarkable-linux-gnueabihf/sysroot/usr/lib/libfuse3.so.3 /opt/x-tools/arm-remarkable-linux-gnueabihf/arm-remarkable-linux-gnueabihf/sysroot/usr/lib/libfuse3.so
	PKG_CONFIG_PATH=$(realpath pkgconfig) cargo build --verbose --release --target=armv7-unknown-linux-gnueabihf || bash
