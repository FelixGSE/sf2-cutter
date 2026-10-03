# Quality gates for sf2-cutter. `make check` is the full PR gate.

FUZZ_SECONDS ?= 120

.PHONY: fmt fmt-check lint test coverage coverage-html mutants fuzz check

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
# PROPTEST_CASES=4: full property-test case counts would multiply across
# hundreds of mutants; a handful of cases per mutant is plenty.
mutants:
	PROPTEST_CASES=4 cargo mutants

# Fuzz the parser and the write/parse round trip (nightly; installs tooling
# on demand). Override duration with FUZZ_SECONDS=...
fuzz:
	rustup toolchain list | grep -q nightly || rustup toolchain install nightly --profile minimal
	command -v cargo-fuzz >/dev/null 2>&1 || cargo install cargo-fuzz --locked
	cargo +nightly fuzz run parse -- -max_total_time=$(FUZZ_SECONDS)
	cargo +nightly fuzz run roundtrip -- -max_total_time=$(FUZZ_SECONDS)

check: fmt-check lint test coverage
