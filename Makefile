test:
	RUST_BACKTRACE=1 python -m unittest test-data/test.py

arm:
	cargo build --release --target=armv7-unknown-linux-gnueabihf

toltec:
	mkdir -p deps
	podman run --name remarkable-build -v deps:/root/.cargo/registry -v .:/project --rm -it  ghcr.io/toltec-dev/rust:v2.1 make -C /project toltec-internal

toltec-internal:
	cargo build --release --target=armv7-unknown-linux-gnueabihf # || bash

target/armv7-unknown-linux-gnueabihf/release/fuse-rm: toltec

deploy-rm: target/armv7-unknown-linux-gnueabihf/release/fuse-rm
	ssh root@remarkable "systemctl stop fuse-rm; \
ln -sf /home/root/.local/share/remarkable/xochitl/ /home/root/rmlibrary"
	scp $? root@remarkable:/opt/bin/
	scp fuse-rm.service root@remarkable:/lib/systemd/system
	ssh root@remarkable systemctl restart fuse-rm
