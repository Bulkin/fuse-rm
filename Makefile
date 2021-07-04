test:
	fusermount -u test-data/target
	cargo run -- test-data/source test-data/target
	ls test-data/target
