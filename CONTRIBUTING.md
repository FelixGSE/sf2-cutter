# Contributing

## Development environment

A [Dev Container](https://containers.dev/) definition is provided under `.devcontainer/`:
a Rust container (rustfmt, clippy, rust-analyzer) running as an unprivileged user with no
sudo, with the quality tooling (`cargo-llvm-cov`, `cargo-mutants`, `cargo-fuzz`,
fluidsynth) baked in and `target/` on a named volume. Any Dev-Containers-compatible
editor or the `devcontainer` CLI works.

Without the container you need: Rust ≥ 1.88 (the declared MSRV), and optionally
fluidsynth on PATH for the audio round-trip tests (they skip when it is absent).

## Quality gates

```sh
make check     # fmt-check, clippy -D warnings (pedantic + complexity ceilings),
               # tests, rustdoc lints, coverage floor (make coverage COVERAGE_MIN=NN)
make mutants   # cargo-mutants; new survivors must be killed or documented below
make fuzz      # libFuzzer over parse + write/parse round trip (nightly, on demand)
cargo test --no-default-features   # the sf3-less build must stay green
cargo test --all-features          # includes the sf3-write Vorbis encoder
cargo test -- --ignored            # real-font tests; needs FluidR3_GM.sf2 in the repo root
```

Known equivalent (unkillable) mutants are documented in the repository's quality notes;
anything new that `make mutants` reports must be killed by a test or justified in the PR.

## Test conventions (mandatory)

- Names: `<subject>_should_<outcome>_when_<condition>`; the `when` clause may be omitted
  only if there is genuinely no condition.
- Every test body is structured with `// given`, `// when`, `// then` comments.
- Tests build small synthetic fonts via `builder::SoundFontBuilder`; nothing outside
  `#[ignore]`d tests may depend on large real-world files.

## Architecture

`src/lib.rs` exposes the library; `src/main.rs` is thin CLI glue (excluded from mutation
testing). Modules:

| Module | Responsibility |
|---|---|
| `riff` | generic RIFF chunk reader/writer |
| `model` | SF2 data model, spec constants, generator names |
| `parse` | bytes → model, structural validation |
| `write` | model → bytes, canonical form |
| `validate` | integrity checks returning issue lists |
| `select` | which presets to keep (patterns, addresses, recipes) |
| `extract` | reachability walk, re-indexing, sample slicing, salvage |
| `merge` | combine fonts with offset re-indexing |
| `edit` | move/rename presets |
| `convert` | sf2 ⇄ sf3 (`sf3-write` feature for compression) |
| `export` | sample WAV export |
| `builder` | programmatic font construction (also the test fixture factory) |
| `sf3` | Ogg-Vorbis decode/encode wrappers (feature-gated) |

Key invariants: `write(parse(x))` round-trips (byte-identical for canonical files) and
every produced font passes `validate` before touching disk. SF3-compressed samples use
BYTE offsets into `smpl` and decoded-relative loop points — anything touching sample
offsets must branch on `SampleHeader::is_compressed()`.
