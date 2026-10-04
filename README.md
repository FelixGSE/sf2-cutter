# sf2-cutter

Cut SoundFont 2 (`.sf2`) files down to the presets you actually need. Point it at a big
GM bank (e.g. Debian's 141 MB `FluidR3_GM.sf2`), pick the sounds you want, and get a small,
valid `.sf2` containing only those presets, the instruments and samples they reach, and
nothing else.

The SF2 parser and writer are hand-rolled (no soundfont crates). A full
parse → write round trip of FluidR3_GM.sf2 is byte-identical.

## Usage

```sh
# What's inside?
sf2-cutter list FluidR3_GM.sf2

# Integrity check (exit code 1 on errors, warnings are tolerated)
sf2-cutter validate FluidR3_GM.sf2

# Extract everything whose name contains "piano" (case-insensitive)
sf2-cutter extract FluidR3_GM.sf2 --match piano -o pianos.sf2

# See what would happen first
sf2-cutter extract FluidR3_GM.sf2 --match piano --dry-run

# Precise MIDI addressing, several criteria combine as a union
sf2-cutter extract FluidR3_GM.sf2 -p 0:0 -p 0:4 --match "*organ*" --keep-drums -o subset.sf2

# Pick interactively (other flags pre-check the list)
sf2-cutter extract FluidR3_GM.sf2 --match piano --interactive -o subset.sf2

# Reusable extraction recipe
sf2-cutter extract FluidR3_GM.sf2 --config recipes/pianos.toml -o pianos.sf2

# Machine-readable output for scripting (list, validate, extract)
sf2-cutter list FluidR3_GM.sf2 --json | jq '.presets[0]'

# Rename the extracted bank
sf2-cutter extract FluidR3_GM.sf2 -m piano --name "Just Pianos" -o pianos.sf2

# Merge several fonts into one (presets keep their bank:prog addresses)
sf2-cutter merge pianos.sf2 drums.sf2 --name "My Rig" -o rig.sf2

# Move and rename presets in the output (works on extract and merge)
sf2-cutter extract FluidR3_GM.sf2 -m piano --move "8:6=0:20" --rename "0:0=Concert Grand" -o out.sf2
```

`--move` applies all moves simultaneously (so two moves can swap addresses) and refuses
contested targets; `--rename` changes a preset's name (19 characters max).

For fonts that fail validation, `extract --force` salvages what is reachable: dangling
instrument/sample references are dropped and out-of-range sample offsets are clamped, and
the output is still guaranteed to validate cleanly.

SF3 fonts (Ogg-Vorbis-compressed, e.g. MuseScore_General.sf3) can be used as extraction
*inputs*: kept samples are decoded to PCM and the output is a plain sf2 playable anywhere
(enabled by the default `sf3` cargo feature; writing SF3 is not supported).

Selection criteria:

- `--match PATTERN` — case-insensitive name match; plain text matches as substring,
  `*` wildcards match the whole name (`"*grand*"`).
- `--preset BANK:PROG` — exact MIDI address (`0:0`; drums live on bank `128`).
- `--config FILE` — TOML recipe, see below.
- `--keep-drums` — also keep every preset on percussion bank 128.
- `--renumber` — compact program numbers per bank starting at 0 (default keeps the
  original numbers so existing MIDI files keep working).

Recipe format (all keys optional, combined with the command-line flags):

```toml
match = ["*piano*", "rhodes"]
presets = ["0:0", "128:0"]
keep_drums = false
renumber = false
```

Extraction notes:

- Only *mutual* stereo links are treated as real pairs and kept together. One-directional
  links (FluidR3 types 970 samples left/right but points all their links at sample 0) are
  treated as broken and sanitised to mono headers in the output.
- Every output is re-validated before it is written; `validate` runs the same checks.
- Outputs carry provenance: `sf2-cutter v<version>` is appended to the `ISFT` tool chain,
  and `--name` replaces the bank name (`INAM`).

## Development

Open the folder in VS Code and choose **Reopen in Container** (Rust devcontainer with
rustfmt, clippy, rust-analyzer, GitHub CLI, Claude Code; runs as unprivileged `dev` user).

```sh
make check     # full gate: fmt-check, clippy -D warnings, tests, coverage floor (80% lines)
make mutants   # mutation testing (slow; run before merging logic changes)
make fuzz      # time-boxed fuzzing of the parser and round trip (nightly)
cargo test -- --ignored   # real-font tests; need FluidR3_GM.sf2 in the repo root
```

The test suite includes property-based tests (random fonts, round-trip and extraction
invariants) and, when fluidsynth is on PATH, renders an extracted font to WAV and asserts
the audio is not silence.

Shell completions and a man page are built in:

```sh
sf2-cutter completions bash > /etc/bash_completion.d/sf2-cutter   # or zsh/fish/...
sf2-cutter man | man -l -
```

Releases are built automatically for Linux (x86_64/aarch64), macOS (arm64), and Windows
when a `v*` tag is pushed. Dependency licenses and advisories are checked in CI with
cargo-deny.

Test conventions: every test is structured with `// given` / `// when` / `// then`
comments and named `<subject>_should_<outcome>_when_<condition>`. Architecture: library
modules `riff`, `model`, `parse`, `write`, `validate`, `select`, `extract`, `merge`,
`edit`, `builder`, and `sf3` (feature-gated); `src/main.rs` is thin CLI glue.
