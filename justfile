@cover:
	cargo tarpaulin --ignore-config --out html --target-dir target/coverage --output-dir target --frozen --no-fail-fast --skip-clean

@test *ARGS:
	cargo nextest run {{ARGS}}

@bacon action='test' package='all':
	#!/usr/bin/env sh
	if [ '{{package}}' == 'all' ]; then
		bacon {{action}}
	else
		bacon {{action}} -- -p {{package}}
	fi
