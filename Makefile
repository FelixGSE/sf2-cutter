# Quality gates for sf2-cutter. `make check` is the full PR gate.

.PHONY: fmt fmt-check lint test coverage coverage-html mutants check

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test

# CARGO_INCREMENTAL=0: incremental codegen hard-links object files, which
# fails with EACCES on virtiofs mounts (macOS/Lima docker); coverage gains
# nothing from incremental builds anyway.
coverage:
	CARGO_INCREMENTAL=0 cargo llvm-cov --all-features --fail-under-lines 80

coverage-html:
	CARGO_INCREMENTAL=0 cargo llvm-cov --all-features --html --open

# Slow; not part of `check`. Run before merging substantial logic changes.
mutants:
	cargo mutants

check: fmt-check lint test coverage
