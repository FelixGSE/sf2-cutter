# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2](https://github.com/FelixGSE/sf2-cutter/compare/v0.1.1...v0.1.2) - 2026-10-10

### Other

- update Cargo.lock dependencies

## [0.1.1](https://github.com/FelixGSE/sf2-cutter/compare/v0.1.0...v0.1.1) - 2026-10-08

### Other

- update Cargo.lock dependencies

## [0.1.0] - 2026-10-04

### Added

- `extract`: pull selected presets into a new, minimal, validated `.sf2` —
  selection by case-insensitive name pattern (`*` globs), `BANK:PROG` address,
  TOML recipe, interactive picker, and `--keep-drums`; options for
  `--renumber`, `--dry-run`, `--name` (INAM), `--move B:P=B:P`,
  `--rename B:P=NAME`, and `--force` salvage mode for broken fonts
- `merge`: combine fonts with full re-indexing, stereo-link preservation, and
  24-bit (`sm24`) zero-fill alignment
- `list` and `validate` with machine-readable `--json` output and
  scripting-friendly exit codes
- SF3 read support (`sf3` feature, default on): Ogg-Vorbis-compressed fonts
  work as extraction inputs; kept samples are decoded to plain PCM
- provenance stamping: outputs append `sf2-cutter v<version>` to the `ISFT`
  tool chain
- `split`: one validated `.sf2` per preset, streamed one at a time, with
  Unicode-preserving filenames and `--force` salvage
- `dump`: the complete font structure as JSON, with generator names and
  decoded values (signed amounts, key/velocity ranges)
- `samples`: export sample audio as WAV, for all samples or a preset
  selection, keeping the input font's sample numbering; `--force` skips
  samples that cannot be exported
- `convert`: sf2 ⇄ sf3 in both directions; compressing uses the vendored
  Vorbis encoder behind the off-by-default `sf3-write` feature
- shell completions (`completions <shell>`) and a man page (`man`)
- release binaries for Linux (static musl, x86_64/aarch64), macOS (arm64),
  and Windows, with sha256 checksums and sf3 compression included
- quality gates: clippy pedantic with complexity ceilings, 80% line-coverage
  floor, mutation testing, libFuzzer targets, property-based tests, and
  fluidsynth round-trip audio verification

[Unreleased]: https://github.com/FelixGSE/sf2-cutter/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/FelixGSE/sf2-cutter/releases/tag/v0.1.0
