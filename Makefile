# Quality gates for sf2-cutter. `make check` is the full PR gate.

FUZZ_SECONDS ?= 120
COVERAGE_MIN ?= 80

.PHONY: fmt fmt-check lint test doc coverage coverage-html mutants fuzz check

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test

doc:
	RUSTDOCFLAGS="-D warnings -D rustdoc::all" cargo doc --no-deps --all-features

# Coverage gains nothing from incremental builds; keep them off there.
# Override the floor with COVERAGE_MIN=NN.
coverage:
	CARGO_INCREMENTAL=0 cargo llvm-cov --all-features --fail-under-lines $(COVERAGE_MIN)

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

check: fmt-check lint test doc coverage
