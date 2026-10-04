//! `sf2-cutter` command line: list, validate, and extract presets from
//! `SoundFont` 2 files. Thin glue over the library; all format logic lives in
//! `sf2_cutter::*`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory as _, Parser, Subcommand};

use sf2_cutter::extract::{self, Options};
use sf2_cutter::model::{Preset, SoundFont};
use sf2_cutter::select::{PresetSpec, Recipe, Selection};
use sf2_cutter::validate::{Issue, Severity, has_errors, validate};
use sf2_cutter::{merge, parse, write};

#[derive(Parser)]
#[command(
    name = "sf2-cutter",
    version,
    about = "Cut SoundFont (.sf2) files down to the presets you need"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List all presets with bank:program, zone count, and sample size
    List {
        /// Input .sf2 file
        input: PathBuf,
        /// Emit machine-readable JSON on stdout
        #[arg(long)]
        json: bool,
    },
    /// Strictly parse and integrity-check a file (exit code 1 on errors)
    Validate {
        /// Input .sf2 file
        input: PathBuf,
        /// Emit machine-readable JSON on stdout
        #[arg(long)]
        json: bool,
    },
    /// Extract selected presets into a new .sf2 file
    Extract {
        /// Input .sf2 file
        input: PathBuf,
        /// Output .sf2 file (required unless --dry-run)
        #[arg(short, long, required_unless_present = "dry_run")]
        output: Option<PathBuf>,
        /// Case-insensitive name pattern; `*` wildcards allowed (repeatable)
        #[arg(short = 'm', long = "match", value_name = "PATTERN")]
        patterns: Vec<String>,
        /// Preset address BANK:PROG, e.g. 0:4 (repeatable)
        #[arg(short = 'p', long = "preset", value_name = "BANK:PROG")]
        presets: Vec<String>,
        /// TOML recipe file (keys: match, presets, `keep_drums`, renumber)
        #[arg(short = 'c', long, value_name = "FILE")]
        config: Option<PathBuf>,
        /// Pick presets interactively (pre-checked with the other criteria)
        #[arg(short = 'i', long)]
        interactive: bool,
        /// Also keep every preset on percussion bank 128
        #[arg(long)]
        keep_drums: bool,
        /// Compact program numbers per bank, starting at 0
        #[arg(long)]
        renumber: bool,
        /// Report what would be kept without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Extract despite input validation errors: dangling references are
        /// dropped and out-of-range sample offsets clamped
        #[arg(long)]
        force: bool,
        /// Rename the output bank (sets the INAM chunk)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Move a kept preset to a new address, e.g. 8:6=0:20 (repeatable;
        /// applied after extraction, all moves simultaneously)
        #[arg(long = "move", value_name = "B:P=B:P")]
        moves: Vec<String>,
        /// Rename a kept preset, e.g. "0:0=Concert Grand" (repeatable)
        #[arg(long = "rename", value_name = "B:P=NAME")]
        renames: Vec<String>,
        /// Emit a machine-readable JSON report on stdout
        #[arg(long)]
        json: bool,
    },
    /// Merge several .sf2 files into one (presets keep their addresses)
    Merge {
        /// Input .sf2 files, merged in order
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
        /// Output .sf2 file
        #[arg(short, long)]
        output: PathBuf,
        /// Rename the output bank (sets the INAM chunk)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Move a preset to a new address, e.g. 8:6=0:20 (repeatable)
        #[arg(long = "move", value_name = "B:P=B:P")]
        moves: Vec<String>,
        /// Rename a preset, e.g. "0:0=Concert Grand" (repeatable)
        #[arg(long = "rename", value_name = "B:P=NAME")]
        renames: Vec<String>,
        /// Emit a machine-readable JSON report on stdout
        #[arg(long)]
        json: bool,
    },
    /// Write every preset to its own .sf2 file
    Split {
        /// Input .sf2 file
        input: PathBuf,
        /// Output directory (created if missing)
        #[arg(short, long, value_name = "DIR")]
        output: PathBuf,
        /// Split despite input validation errors: dangling references are
        /// dropped and out-of-range sample offsets clamped per preset
        #[arg(long)]
        force: bool,
        /// Emit a machine-readable JSON report on stdout
        #[arg(long)]
        json: bool,
    },
    /// Dump the complete font structure as JSON on stdout
    Dump {
        /// Input .sf2 file
        input: PathBuf,
    },
    /// Export sample audio as 16-bit mono WAV files
    Samples {
        /// Input .sf2 file
        input: PathBuf,
        /// Output directory (created if missing)
        #[arg(short, long, value_name = "DIR")]
        output: PathBuf,
        /// Only samples reachable from presets matching this pattern (repeatable)
        #[arg(short = 'm', long = "match", value_name = "PATTERN")]
        patterns: Vec<String>,
        /// Only samples reachable from this preset address (repeatable)
        #[arg(short = 'p', long = "preset", value_name = "BANK:PROG")]
        presets: Vec<String>,
        /// Export despite validation errors, skipping (and reporting) samples
        /// that cannot be exported instead of aborting
        #[arg(long)]
        force: bool,
        /// Emit a machine-readable JSON report on stdout
        #[arg(long)]
        json: bool,
    },
    /// Convert between plain sf2 and compressed sf3 (Ogg Vorbis)
    Convert {
        /// Input .sf2 or .sf3 file
        input: PathBuf,
        /// Output file; the target format is inferred from its extension
        #[arg(short, long)]
        output: PathBuf,
        /// Target format; required when the output extension is not
        /// .sf2/.sf3, and must agree with it when it is
        #[arg(long, value_enum)]
        to: Option<TargetFormat>,
        /// Vorbis quality for sf3 output: 0.0 (smallest) ..= 1.0 (best)
        #[arg(long, default_value_t = 0.4)]
        quality: f32,
    },
    /// Generate shell completions on stdout
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Print the man page (roff) on stdout
    #[command(hide = true)]
    Man,
}

/// Conversion target for the `convert` subcommand.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum TargetFormat {
    /// Plain PCM `SoundFont`
    Sf2,
    /// Ogg-Vorbis-compressed `SoundFont` (`MuseScore` convention)
    Sf3,
}

type CliResult = Result<ExitCode, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            emit_err(&format!("error: {error}\n"));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> CliResult {
    match cli.command {
        Command::List { input, json } => cmd_list(&input, json),
        Command::Validate { input, json } => cmd_validate(&input, json),
        Command::Extract {
            input,
            output,
            patterns,
            presets,
            config,
            interactive,
            keep_drums,
            renumber,
            dry_run,
            force,
            name,
            moves,
            renames,
            json,
        } => cmd_extract(&ExtractArgs {
            input,
            output,
            patterns,
            presets,
            config,
            interactive,
            keep_drums,
            renumber,
            dry_run,
            force,
            name,
            moves,
            renames,
            json,
        }),
        Command::Merge {
            inputs,
            output,
            name,
            moves,
            renames,
            json,
        } => cmd_merge(&MergeArgs {
            inputs,
            output,
            name,
            moves,
            renames,
            json,
        }),
        Command::Split {
            input,
            output,
            force,
            json,
        } => cmd_split(&input, &output, force, json),
        Command::Dump { input } => cmd_dump(&input),
        Command::Samples {
            input,
            output,
            patterns,
            presets,
            force,
            json,
        } => cmd_samples(&SamplesArgs {
            input,
            output,
            patterns,
            presets,
            force,
            json,
        }),
        Command::Convert {
            input,
            output,
            to,
            quality,
        } => cmd_convert(&input, &output, to, quality),
        Command::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "sf2-cutter",
                &mut std::io::stdout(),
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Man => {
            let mut page = Vec::new();
            clap_mangen::Man::new(Cli::command()).render(&mut page)?;
            emit(&String::from_utf8_lossy(&page))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn load_font(path: &Path) -> Result<(SoundFont, u64), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let font = parse::parse(&bytes)?;
    Ok((font, bytes.len() as u64))
}

/// Preset indices sorted by (bank, program) for stable, readable listings.
fn sorted_preset_indices(font: &SoundFont) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..font.presets.len()).collect();
    indices.sort_by_key(|&i| (font.presets[i].bank, font.presets[i].program, i));
    indices
}

fn preset_label(preset: &Preset) -> String {
    format!(
        "{:>4}:{:<3} {:<20}",
        preset.bank,
        preset.program,
        preset.name.to_display()
    )
}

/// Writes to stdout, treating a closed pipe (e.g. `sf2-cutter list | head`)
/// as success instead of panicking the way `println!` would.
fn emit(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// One preset row in JSON reports.
#[derive(serde::Serialize)]
struct PresetJson {
    bank: u16,
    program: u16,
    name: String,
    zones: usize,
    sample_bytes: u64,
}

#[derive(serde::Serialize)]
struct TotalsJson {
    presets: usize,
    instruments: usize,
    samples: usize,
    file_size: u64,
}

#[derive(serde::Serialize)]
struct ListJson {
    name: Option<String>,
    presets: Vec<PresetJson>,
    totals: TotalsJson,
}

#[derive(serde::Serialize)]
struct ValidateJson {
    ok: bool,
    errors: usize,
    warnings: usize,
    issues: Vec<Issue>,
}

#[derive(serde::Serialize)]
struct ExtractJson {
    kept: Vec<PresetJson>,
    instruments: usize,
    samples: usize,
    input_size: u64,
    predicted_size: u64,
    written: Option<String>,
}

fn preset_json(font: &SoundFont, index: usize) -> PresetJson {
    let preset = &font.presets[index];
    PresetJson {
        bank: preset.bank,
        program: preset.program,
        name: preset.name.to_display(),
        zones: preset.zones.len(),
        sample_bytes: extract::preset_sample_bytes(font, index),
    }
}

fn list_json(font: &SoundFont, file_size: u64) -> ListJson {
    ListJson {
        name: font.name(),
        presets: sorted_preset_indices(font)
            .into_iter()
            .map(|index| preset_json(font, index))
            .collect(),
        totals: TotalsJson {
            presets: font.presets.len(),
            instruments: font.instruments.len(),
            samples: font.samples.len(),
            file_size,
        },
    }
}

fn validate_json(issues: &[Issue]) -> ValidateJson {
    let errors = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    ValidateJson {
        ok: errors == 0,
        errors,
        warnings: issues.len() - errors,
        issues: issues.to_vec(),
    }
}

fn emit_json<T: serde::Serialize>(value: &T) -> Result<(), Box<dyn std::error::Error>> {
    emit(&format!("{}\n", serde_json::to_string_pretty(value)?))
}

/// Dumps the full structure of a parseable font; deliberately no validation
/// gate, so broken fonts can be inspected too.
fn cmd_dump(input: &Path) -> CliResult {
    let (font, _) = load_font(input)?;
    emit_json(&font)?;
    Ok(ExitCode::SUCCESS)
}

fn cmd_list(input: &Path, json: bool) -> CliResult {
    let (font, file_size) = load_font(input)?;
    if json {
        emit_json(&list_json(&font, file_size))?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut buf = String::new();
    let _ = writeln!(
        buf,
        "{}",
        font.name().unwrap_or_else(|| "(unnamed font)".into())
    );
    let _ = writeln!(buf, "bank:prog name                 zones  est. sample KB");
    for index in sorted_preset_indices(&font) {
        let preset = &font.presets[index];
        let kib = extract::preset_sample_bytes(&font, index).div_ceil(1024);
        let _ = writeln!(
            buf,
            "{} {:>5} {:>15}",
            preset_label(preset),
            preset.zones.len(),
            kib
        );
    }
    let _ = writeln!(
        buf,
        "{} presets, {} instruments, {} samples, {} bytes",
        font.presets.len(),
        font.instruments.len(),
        font.samples.len(),
        file_size
    );
    emit(&buf)?;
    Ok(ExitCode::SUCCESS)
}

/// Issues shown individually before the rest is summarised; keeps fonts with
/// thousands of findings (hello, Fluid R3) from flooding the terminal.
const MAX_PRINTED_ISSUES: usize = 20;

/// Writes to stderr, ignoring write failures (a dead stderr leaves nowhere
/// to report to, and `eprintln!` would panic on a closed pipe).
fn emit_err(text: &str) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(text.as_bytes());
    let _ = err.flush();
}

fn print_issues(issues: &[Issue]) {
    for issue in issues.iter().take(MAX_PRINTED_ISSUES) {
        emit_err(&format!("{issue}\n"));
    }
    if issues.len() > MAX_PRINTED_ISSUES {
        let errors = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count();
        emit_err(&format!(
            "... and {} more ({} errors, {} warnings in total)\n",
            issues.len() - MAX_PRINTED_ISSUES,
            errors,
            issues.len() - errors
        ));
    }
}

fn cmd_validate(input: &Path, json: bool) -> CliResult {
    let (font, _) = load_font(input)?;
    let issues = validate(&font);
    if json {
        emit_json(&validate_json(&issues))?;
        return Ok(if has_errors(&issues) {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    print_issues(&issues);
    if has_errors(&issues) {
        emit_err(&format!("INVALID: {}\n", input.display()));
        return Ok(ExitCode::FAILURE);
    }
    emit(&format!(
        "OK: {} ({} presets, {} instruments, {} samples)\n",
        input.display(),
        font.presets.len(),
        font.instruments.len(),
        font.samples.len()
    ))?;
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::struct_excessive_bools)] // one bool per CLI switch
struct ExtractArgs {
    input: PathBuf,
    output: Option<PathBuf>,
    patterns: Vec<String>,
    presets: Vec<String>,
    config: Option<PathBuf>,
    interactive: bool,
    keep_drums: bool,
    renumber: bool,
    dry_run: bool,
    force: bool,
    name: Option<String>,
    moves: Vec<String>,
    renames: Vec<String>,
    json: bool,
}

struct MergeArgs {
    inputs: Vec<PathBuf>,
    output: PathBuf,
    name: Option<String>,
    moves: Vec<String>,
    renames: Vec<String>,
    json: bool,
}

/// Parses `B:P=B:P` into a (from, to) move.
fn parse_move(text: &str) -> Result<(PresetSpec, PresetSpec), Box<dyn std::error::Error>> {
    let (from, to) = text
        .split_once('=')
        .ok_or_else(|| format!("invalid --move `{text}` (expected B:P=B:P)"))?;
    Ok((from.trim().parse()?, to.trim().parse()?))
}

/// Parses `B:P=NAME` into an (address, name) rename.
fn parse_rename(text: &str) -> Result<(PresetSpec, String), Box<dyn std::error::Error>> {
    let (spec, name) = text
        .split_once('=')
        .ok_or_else(|| format!("invalid --rename `{text}` (expected B:P=NAME)"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("invalid --rename `{text}`: empty name").into());
    }
    Ok((spec.trim().parse()?, name.to_string()))
}

/// Applies `--move` and `--rename` options to a result font.
fn apply_edits(
    font: &mut SoundFont,
    moves: &[String],
    renames: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let moves = moves
        .iter()
        .map(|text| parse_move(text))
        .collect::<Result<Vec<_>, _>>()?;
    if !moves.is_empty() {
        sf2_cutter::edit::remap_presets(font, &moves)?;
    }
    for text in renames {
        let (spec, name) = parse_rename(text)?;
        sf2_cutter::edit::rename_preset(font, spec, &name)?;
    }
    Ok(())
}

fn build_selection(args: &ExtractArgs) -> Result<(Selection, Options), Box<dyn std::error::Error>> {
    let mut selection = Selection::new();
    let mut options = Options {
        renumber: args.renumber,
        rename: args.name.clone(),
        salvage: args.force,
    };
    for pattern in &args.patterns {
        selection.add_pattern(pattern);
    }
    for spec in &args.presets {
        selection.add_spec(spec.parse()?);
    }
    if let Some(config) = &args.config {
        let text =
            std::fs::read_to_string(config).map_err(|e| format!("{}: {e}", config.display()))?;
        let recipe = Recipe::from_toml_str(&text)?;
        options.renumber = options.renumber || recipe.renumber;
        selection.apply_recipe(&recipe)?;
    }
    if args.keep_drums {
        selection.set_keep_drums(true);
    }
    Ok((selection, options))
}

/// Shows a multi-select of all presets, pre-checked with `selection`, and
/// returns a selection of exactly the picked presets.
fn pick_interactively(
    font: &SoundFont,
    selection: &Selection,
) -> Result<Selection, Box<dyn std::error::Error>> {
    let order = sorted_preset_indices(font);
    let labels: Vec<String> = order
        .iter()
        .map(|&i| preset_label(&font.presets[i]))
        .collect();
    let defaults: Vec<bool> = order
        .iter()
        .map(|&i| selection.matches(i, &font.presets[i]))
        .collect();
    let picked = dialoguer::MultiSelect::new()
        .with_prompt("Select presets to keep (space toggles, enter confirms)")
        .items(&labels)
        .defaults(&defaults)
        .interact()?;
    let mut result = Selection::new();
    for position in picked {
        result.add_index(order[position]);
    }
    Ok(result)
}

fn cmd_extract(args: &ExtractArgs) -> CliResult {
    let (font, input_size) = load_font(&args.input)?;
    let input_issues = validate(&font);
    print_issues(&input_issues);
    if has_errors(&input_issues) {
        if args.force {
            emit_err("input font fails validation; salvaging what is reachable (--force)\n");
        } else {
            return Err("input font fails validation; aborting (use --force to salvage)".into());
        }
    }

    let (mut selection, options) = build_selection(args)?;
    if args.interactive {
        selection = pick_interactively(&font, &selection)?;
        if selection.is_empty() {
            return Err("no presets picked; aborting".into());
        }
    } else if selection.is_empty() {
        return Err("no selection criteria; use --match/--preset/--config/--interactive".into());
    }

    let mut result = extract::extract(&font, &selection, &options)?;
    apply_edits(&mut result, &args.moves, &args.renames)?;
    let output_issues = validate(&result);
    if has_errors(&output_issues) {
        print_issues(&output_issues);
        return Err("internal error: extracted font fails validation; not writing".into());
    }

    let predicted_size = write::file_size(&result);
    if args.json {
        return extract_json_report(args, &result, input_size, predicted_size);
    }
    let mut buf = String::new();
    let _ = writeln!(
        buf,
        "keeping {} of {} presets:",
        result.presets.len(),
        font.presets.len()
    );
    for preset in &result.presets {
        let _ = writeln!(buf, "  {}", preset_label(preset));
    }
    let _ = writeln!(
        buf,
        "instruments: {} of {}, samples: {} of {}",
        result.instruments.len(),
        font.instruments.len(),
        result.samples.len(),
        font.samples.len()
    );
    let _ = writeln!(buf, "size: {input_size} -> {predicted_size} bytes");
    emit(&buf)?;

    if args.dry_run {
        emit("dry run: nothing written\n")?;
        return Ok(ExitCode::SUCCESS);
    }
    let output = args.output.as_ref().ok_or("missing --output")?;
    write_output(output, &result)?;
    emit(&format!("wrote {}\n", output.display()))?;
    Ok(ExitCode::SUCCESS)
}

/// Serialises and writes a font via a sibling temp file and rename, so a
/// failed write never leaves a truncated destination (and `-o <input>` stays
/// safe: the input was fully read before this point).
fn write_output(output: &Path, font: &SoundFont) -> Result<u64, Box<dyn std::error::Error>> {
    let bytes = write::write(font)?;
    write_bytes(output, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Writes bytes via a sibling temp file and rename (same guarantees as
/// `write_output`, for non-font payloads).
fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("cannot move {} to {}: {e}", tmp.display(), path.display()))?;
    Ok(())
}

/// Builds the shared JSON report for extract and merge results.
fn result_json(
    result: &SoundFont,
    input_size: u64,
    predicted_size: u64,
    written: Option<String>,
) -> ExtractJson {
    ExtractJson {
        kept: (0..result.presets.len())
            .map(|index| preset_json(result, index))
            .collect(),
        instruments: result.instruments.len(),
        samples: result.samples.len(),
        input_size,
        predicted_size,
        written,
    }
}

/// Writes the extract result (and optionally the output file) as JSON.
fn extract_json_report(
    args: &ExtractArgs,
    result: &SoundFont,
    input_size: u64,
    predicted_size: u64,
) -> CliResult {
    let written = if args.dry_run {
        None
    } else {
        let output = args.output.as_ref().ok_or("missing --output")?;
        write_output(output, result)?;
        Some(output.display().to_string())
    };
    emit_json(&result_json(result, input_size, predicted_size, written))?;
    Ok(ExitCode::SUCCESS)
}

/// Filesystem-safe slug of a preset name: alphanumerics of any script plus
/// `-` and `_` are kept, runs of anything else — including every character
/// Windows forbids in filenames — collapse to a single `-`. Lowercasing is
/// full Unicode so that names differing only in case produce the SAME stem
/// (and thus get collision suffixes) instead of silently overwriting each
/// other on case-insensitive filesystems. Empty results fall back to
/// `fallback`.
fn slug(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for character in name.chars() {
        if character.is_alphanumeric() || character == '-' || character == '_' {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.extend(character.to_lowercase());
        } else {
            gap = true;
        }
    }
    if out.is_empty() { fallback.into() } else { out }
}

/// Reserves `stem` in `used`, appending `-2`, `-3`, ... on collisions.
fn unique_stem(used: &mut std::collections::HashSet<String>, stem: &str) -> String {
    if used.insert(stem.to_string()) {
        return stem.to_string();
    }
    let mut counter = 2u32;
    loop {
        let candidate = format!("{stem}-{counter}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        counter += 1;
    }
}

#[derive(serde::Serialize)]
struct SplitEntry {
    bank: u16,
    program: u16,
    name: String,
    file: String,
    size: u64,
    instruments: usize,
    samples: usize,
}

/// Writes every preset of the input into its own .sf2 under `output`,
/// streaming one part at a time (peak memory stays at one preset's worth).
fn cmd_split(input: &Path, output: &Path, force: bool, json: bool) -> CliResult {
    let (font, _) = load_font(input)?;
    let issues = validate(&font);
    print_issues(&issues);
    if has_errors(&issues) {
        if force {
            emit_err("input font fails validation; salvaging what is reachable (--force)\n");
        } else {
            return Err("input font fails validation; aborting (use --force to salvage)".into());
        }
    }

    prepare_output_dir(output)?;

    let options = Options {
        salvage: force,
        ..Options::default()
    };
    let total = font.presets.len();
    let mut used = std::collections::HashSet::new();
    let mut entries: Vec<SplitEntry> = Vec::with_capacity(total);
    for part in extract::split_presets(&font, &options) {
        let progress = format!("after writing {} of {total} files", entries.len());
        let single = part.map_err(|e| format!("{progress}: {e}"))?;
        let output_issues = validate(&single);
        if has_errors(&output_issues) {
            print_issues(&output_issues);
            return Err(format!("internal error: invalid part {progress}; not writing it").into());
        }
        let Some(preset) = single.presets.first() else {
            return Err(format!("internal error: empty part {progress}").into());
        };
        let stem = unique_stem(
            &mut used,
            &format!(
                "{:03}-{:03}-{}",
                preset.bank,
                preset.program,
                slug(&preset.name.to_display(), "preset")
            ),
        );
        let path = output.join(format!("{stem}.sf2"));
        let size = write_output(&path, &single).map_err(|e| format!("{progress}: {e}"))?;
        entries.push(SplitEntry {
            bank: preset.bank,
            program: preset.program,
            name: preset.name.to_display(),
            file: path.display().to_string(),
            size,
            instruments: single.instruments.len(),
            samples: single.samples.len(),
        });
    }

    if json {
        emit_json(&serde_json::json!({ "count": entries.len(), "written": entries }))?;
    } else {
        let mut buf = String::new();
        for entry in &entries {
            let _ = writeln!(buf, "wrote {}", entry.file);
        }
        let _ = writeln!(
            buf,
            "split {} presets into {}",
            entries.len(),
            output.display()
        );
        emit(&buf)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Creates the output directory and warns when it already holds files, since
/// same-named outputs are overwritten and unrelated ones are left in place.
fn prepare_output_dir(output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let occupied = std::fs::read_dir(output).is_ok_and(|mut dir| dir.next().is_some());
    std::fs::create_dir_all(output)
        .map_err(|e| format!("cannot create {}: {e}", output.display()))?;
    if occupied {
        emit_err(&format!(
            "warning: {} is not empty; same-named files are overwritten, others left in place\n",
            output.display()
        ));
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct SampleEntry {
    index: usize,
    name: String,
    file: String,
    frames: usize,
    sample_rate: u32,
}

#[derive(serde::Serialize)]
struct SkippedSample {
    index: usize,
    name: String,
    reason: String,
}

struct SamplesArgs {
    input: PathBuf,
    output: PathBuf,
    patterns: Vec<String>,
    presets: Vec<String>,
    force: bool,
    json: bool,
}

/// Exports sample audio as WAV files: every sample, or only those reachable
/// from a preset selection. Indices (filename prefix and report) always refer
/// to the input font, so they match `dump`/`validate`.
fn cmd_samples(args: &SamplesArgs) -> CliResult {
    let (font, _) = load_font(&args.input)?;
    let issues = validate(&font);
    print_issues(&issues);
    if has_errors(&issues) {
        if args.force {
            emit_err("input font fails validation; exporting what is possible (--force)\n");
        } else {
            return Err(
                "input font fails validation; aborting (use --force to skip bad samples)".into(),
            );
        }
    }

    let mut selection = Selection::new();
    for pattern in &args.patterns {
        selection.add_pattern(pattern);
    }
    for spec in &args.presets {
        selection.add_spec(spec.parse()?);
    }
    let options = Options {
        salvage: args.force,
        ..Options::default()
    };
    let indices: Vec<usize> = if selection.is_empty() {
        (0..font.samples.len()).collect()
    } else {
        extract::reachable_sample_indices(&font, &selection, &options)?
    };
    if font.sm24_usable() {
        emit_err("note: the font carries 24-bit (sm24) detail; WAVs are exported as 16-bit\n");
    }

    prepare_output_dir(&args.output)?;
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    let mut skipped_rom = 0usize;
    for index in indices {
        let Some(sample) = font.samples.get(index) else {
            continue; // unreachable: indices come from this font
        };
        if sample.is_rom() {
            skipped_rom += 1;
            continue;
        }
        let progress = format!("after writing {} WAV files", entries.len());
        let wav = match sf2_cutter::export::sample_wav(&font, index) {
            Ok(wav) => wav,
            Err(error) if args.force => {
                emit_err(&format!("warning: skipping sample {index}: {error}\n"));
                skipped.push(SkippedSample {
                    index,
                    name: sample.name.to_display(),
                    reason: error.to_string(),
                });
                continue;
            }
            Err(error) => {
                return Err(format!("{progress}: {error} (use --force to skip it)").into());
            }
        };
        // The zero-padded index is unique per sample, so no collision handling.
        let stem = format!("{index:04}-{}", slug(&sample.name.to_display(), "sample"));
        let path = args.output.join(format!("{stem}.wav"));
        write_bytes(&path, &wav.bytes).map_err(|e| format!("{progress}: {e}"))?;
        entries.push(SampleEntry {
            index,
            name: sample.name.to_display(),
            file: path.display().to_string(),
            frames: wav.frames,
            sample_rate: sample.sample_rate,
        });
    }

    if args.json {
        emit_json(&serde_json::json!({
            "count": entries.len(),
            "skipped_rom": skipped_rom,
            "skipped": skipped,
            "written": entries,
        }))?;
    } else {
        let mut buf = String::new();
        for entry in &entries {
            let _ = writeln!(buf, "wrote {}", entry.file);
        }
        let _ = writeln!(
            buf,
            "exported {} samples to {} ({skipped_rom} ROM samples skipped, {} failed and skipped)",
            entries.len(),
            args.output.display(),
            skipped.len()
        );
        emit(&buf)?;
    }
    Ok(ExitCode::SUCCESS)
}

const fn target_name(target: TargetFormat) -> &'static str {
    match target {
        TargetFormat::Sf2 => "sf2",
        TargetFormat::Sf3 => "sf3",
    }
}

/// Infers the conversion target from the output extension.
fn infer_target(output: &Path) -> Option<TargetFormat> {
    match output.extension()?.to_str()? {
        ext if ext.eq_ignore_ascii_case("sf2") => Some(TargetFormat::Sf2),
        ext if ext.eq_ignore_ascii_case("sf3") => Some(TargetFormat::Sf3),
        _ => None,
    }
}

#[cfg(feature = "sf3-write")]
fn compress_font(font: &SoundFont, quality: f32) -> Result<SoundFont, Box<dyn std::error::Error>> {
    Ok(sf2_cutter::convert::compress(font, quality)?)
}

#[cfg(not(feature = "sf3-write"))]
fn compress_font(
    _font: &SoundFont,
    _quality: f32,
) -> Result<SoundFont, Box<dyn std::error::Error>> {
    Err("this build lacks the `sf3-write` feature; rebuild with --features sf3-write".into())
}

#[cfg(feature = "sf3")]
fn decompress_font(font: &SoundFont) -> Result<SoundFont, Box<dyn std::error::Error>> {
    Ok(sf2_cutter::convert::decompress(font)?)
}

#[cfg(not(feature = "sf3"))]
fn decompress_font(_font: &SoundFont) -> Result<SoundFont, Box<dyn std::error::Error>> {
    Err("this build lacks the `sf3` feature; rebuild with default features".into())
}

/// Converts a font between plain sf2 and compressed sf3.
fn cmd_convert(input: &Path, output: &Path, to: Option<TargetFormat>, quality: f32) -> CliResult {
    let inferred = infer_target(output);
    if let (Some(requested), Some(implied)) = (to, inferred)
        && requested != implied
    {
        return Err(format!(
            "--to {} contradicts the output extension of {}",
            target_name(requested),
            output.display()
        )
        .into());
    }
    let Some(target) = to.or(inferred) else {
        return Err("cannot infer the target format; pass --to sf2|sf3".into());
    };
    if !(0.0..=1.0).contains(&quality) {
        return Err(format!("--quality {quality} outside 0.0..=1.0").into());
    }
    let (font, input_size) = load_font(input)?;
    let issues = validate(&font);
    print_issues(&issues);
    if has_errors(&issues) {
        return Err("input font fails validation; aborting".into());
    }

    let result = match target {
        TargetFormat::Sf3 => compress_font(&font, quality)?,
        TargetFormat::Sf2 => decompress_font(&font)?,
    };
    let output_issues = validate(&result);
    if has_errors(&output_issues) {
        print_issues(&output_issues);
        return Err("internal error: converted font fails validation; not writing".into());
    }
    let predicted_size = write::file_size(&result);
    write_output(output, &result)?;
    emit(&format!(
        "converted {} -> {}\nsize: {input_size} -> {predicted_size} bytes\n",
        input.display(),
        output.display()
    ))?;
    Ok(ExitCode::SUCCESS)
}

/// Loads, validates, merges, re-validates, and writes several fonts.
fn cmd_merge(args: &MergeArgs) -> CliResult {
    let mut fonts = Vec::with_capacity(args.inputs.len());
    let mut input_size = 0;
    for input in &args.inputs {
        let (font, size) = load_font(input)?;
        let issues = validate(&font);
        print_issues(&issues);
        if has_errors(&issues) {
            return Err(format!("{} fails validation; aborting", input.display()).into());
        }
        fonts.push(font);
        input_size += size;
    }

    let mut result = merge::merge(&fonts, args.name.as_deref())?;
    apply_edits(&mut result, &args.moves, &args.renames)?;
    let output_issues = validate(&result);
    if has_errors(&output_issues) {
        print_issues(&output_issues);
        return Err("internal error: merged font fails validation; not writing".into());
    }
    let predicted_size = write::file_size(&result);
    write_output(&args.output, &result)?;
    if args.json {
        emit_json(&result_json(
            &result,
            input_size,
            predicted_size,
            Some(args.output.display().to_string()),
        ))?;
    } else {
        emit(&format!(
            "merged {} fonts: {} presets, {} instruments, {} samples\nsize: {input_size} -> {predicted_size} bytes\nwrote {}\n",
            fonts.len(),
            result.presets.len(),
            result.instruments.len(),
            result.samples.len(),
            args.output.display()
        ))?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sf2_cutter::model::FixedName;

    fn preset(bank: u16, program: u16) -> Preset {
        Preset {
            name: FixedName::from_text("t"),
            program,
            bank,
            library: 0,
            genre: 0,
            morphology: 0,
            zones: vec![],
        }
    }

    fn extract_args() -> ExtractArgs {
        ExtractArgs {
            input: PathBuf::new(),
            output: None,
            patterns: vec![],
            presets: vec![],
            config: None,
            interactive: false,
            keep_drums: false,
            renumber: false,
            dry_run: true,
            force: false,
            name: None,
            moves: vec![],
            renames: vec![],
            json: false,
        }
    }

    #[test]
    fn build_selection_should_keep_recipe_drums_when_cli_flag_absent() {
        // given: a recipe enabling keep_drums, no --keep-drums flag
        let path = std::env::temp_dir().join("sf2-cutter-test-keep-drums.toml");
        std::fs::write(&path, "keep_drums = true\n").unwrap();
        let args = ExtractArgs {
            config: Some(path.clone()),
            ..extract_args()
        };

        // when
        let (selection, _) = build_selection(&args).unwrap();
        std::fs::remove_file(&path).ok();

        // then
        assert!(selection.matches(0, &preset(128, 0)));
    }

    #[test]
    fn build_selection_should_enable_renumber_when_recipe_sets_it() {
        // given
        let path = std::env::temp_dir().join("sf2-cutter-test-renumber.toml");
        std::fs::write(&path, "renumber = true\n").unwrap();
        let args = ExtractArgs {
            config: Some(path.clone()),
            ..extract_args()
        };

        // when
        let (_, options) = build_selection(&args).unwrap();
        std::fs::remove_file(&path).ok();

        // then
        assert!(options.renumber);
    }

    #[test]
    fn build_selection_should_union_flags_when_recipe_also_given() {
        // given: pattern via flag, spec via recipe
        let path = std::env::temp_dir().join("sf2-cutter-test-union.toml");
        std::fs::write(&path, "presets = [\"8:14\"]\n").unwrap();
        let args = ExtractArgs {
            patterns: vec!["piano".into()],
            config: Some(path.clone()),
            keep_drums: true,
            ..extract_args()
        };

        // when
        let (selection, _) = build_selection(&args).unwrap();
        std::fs::remove_file(&path).ok();

        // then: all three criteria are active
        let named = Preset {
            name: FixedName::from_text("Grand Piano"),
            ..preset(0, 0)
        };
        assert!(selection.matches(0, &named));
        assert!(selection.matches(1, &preset(8, 14)));
        assert!(selection.matches(2, &preset(128, 40)));
    }

    #[test]
    fn slug_should_collapse_special_characters_when_name_is_messy() {
        // given / when / then
        assert_eq!(slug("Yamaha Grand Piano", "preset"), "yamaha-grand-piano");
        assert_eq!(slug("E.Piano (bright)!", "preset"), "e-piano-bright");
        assert_eq!(slug("snare_2-alt", "preset"), "snare_2-alt");
        assert_eq!(slug("Flöte", "preset"), "flöte");
        assert_eq!(slug("FLÖTE", "preset"), "flöte"); // full case fold, not just ASCII
        assert_eq!(slug("ピアノ 2", "preset"), "ピアノ-2");
        assert_eq!(slug("a<b>c:d", "preset"), "a-b-c-d");
    }

    #[test]
    fn slug_should_fall_back_when_name_has_no_usable_characters() {
        // given / when / then
        assert_eq!(slug("", "preset"), "preset");
        assert_eq!(slug("!!!", "preset"), "preset");
        assert_eq!(slug("", "sample"), "sample");
    }

    #[test]
    fn unique_stem_should_append_counter_when_stem_collides() {
        // given
        let mut used = std::collections::HashSet::new();

        // when / then
        assert_eq!(unique_stem(&mut used, "000-000-piano"), "000-000-piano");
        assert_eq!(unique_stem(&mut used, "000-000-piano"), "000-000-piano-2");
        assert_eq!(unique_stem(&mut used, "000-000-piano"), "000-000-piano-3");
    }

    #[test]
    fn infer_target_should_match_extension_when_recognisable() {
        // given / when / then
        assert!(matches!(
            infer_target(Path::new("x/out.sf3")),
            Some(TargetFormat::Sf3)
        ));
        assert!(matches!(
            infer_target(Path::new("OUT.SF2")),
            Some(TargetFormat::Sf2)
        ));
        assert!(infer_target(Path::new("out.wav")).is_none());
        assert!(infer_target(Path::new("noext")).is_none());
    }

    #[test]
    fn parse_move_should_split_source_and_target_when_text_is_valid() {
        // given / when
        let (from, to) = parse_move("8:6=0:20").unwrap();

        // then
        assert_eq!((from.bank, from.program), (8, 6));
        assert_eq!((to.bank, to.program), (0, 20));
    }

    #[test]
    fn parse_move_should_fail_when_separator_is_missing() {
        // given / when
        let result = parse_move("8:6");

        // then
        assert!(result.is_err());
    }

    #[test]
    fn parse_rename_should_keep_spaces_in_name_when_text_is_valid() {
        // given / when
        let (spec, name) = parse_rename("0:0=Concert Grand").unwrap();

        // then
        assert_eq!((spec.bank, spec.program), (0, 0));
        assert_eq!(name, "Concert Grand");
    }

    #[test]
    fn parse_rename_should_fail_when_name_is_empty() {
        // given / when
        let result = parse_rename("0:0=  ");

        // then
        assert!(result.is_err());
    }

    #[test]
    fn list_json_should_sort_presets_and_fill_totals_when_building_report() {
        // given: two presets added out of bank:program order
        let mut builder = sf2_cutter::builder::SoundFontBuilder::new("JSON Font");
        builder.add_preset("Later", 1, 0, vec![]);
        builder.add_preset("Earlier", 0, 3, vec![]);
        let font = builder.build();

        // when
        let report = list_json(&font, 1234);

        // then
        assert_eq!(report.name.as_deref(), Some("JSON Font"));
        assert_eq!(report.presets[0].name, "Earlier");
        assert_eq!(report.presets[1].name, "Later");
        assert_eq!(report.totals.presets, 2);
        assert_eq!(report.totals.file_size, 1234);
    }

    #[test]
    fn validate_json_should_count_severities_when_issues_mixed() {
        // given
        let issues = vec![
            sf2_cutter::validate::Issue {
                severity: Severity::Error,
                message: "bad".into(),
            },
            sf2_cutter::validate::Issue {
                severity: Severity::Warning,
                message: "meh".into(),
            },
        ];

        // when
        let report = validate_json(&issues);

        // then
        assert!(!report.ok);
        assert_eq!(report.errors, 1);
        assert_eq!(report.warnings, 1);
        let rendered = serde_json::to_string(&report).unwrap();
        assert!(
            rendered.contains("\"severity\": \"error\"")
                || rendered.contains("\"severity\":\"error\"")
        );
    }
}
