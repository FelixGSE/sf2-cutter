# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- shell completions (`completions <shell>`) and a man page (`man`)
- quality gates: clippy pedantic with complexity ceilings, 80% line-coverage
  floor, mutation testing, libFuzzer targets, property-based tests, and
  fluidsynth round-trip audio verification

### In review

- `split` — one `.sf2` per preset (#5)
- `dump` — full structural JSON with named generators (#6)
- `samples` — WAV export of sample audio (#7)
- `convert` — sf2 ⇄ sf3 with the vendored Vorbis encoder behind the
  off-by-default `sf3-write` feature (#8)

## [0.1.0] — unreleased

Initial development version; `v0.1.0` will be the first tagged release and
this section will absorb the entries above at that point.
