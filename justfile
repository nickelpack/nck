cover:
	cargo tarpaulin --ignore-config --out html --target-dir target/coverage --output-dir target --frozen --no-fail-fast --skip-clean

test:
	cargo nextest run
