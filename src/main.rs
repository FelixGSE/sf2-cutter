//! `sf2-cutter` command line: list, validate, and extract presets from
//! `SoundFont` 2 files. Thin glue over the library; all format logic lives in
//! `sf2_cutter::*`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
        return Err("input font fails validation; aborting".into());
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
        return extract_json_report(args, &font, &result, input_size, predicted_size);
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
fn write_output(output: &Path, font: &SoundFont) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = write::write(font)?;
    let mut tmp_name = output.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, output)
        .map_err(|e| format!("cannot move {} to {}: {e}", tmp.display(), output.display()))?;
    Ok(())
}

/// Writes the extract result (and optionally the output file) as JSON.
fn extract_json_report(
    args: &ExtractArgs,
    font: &SoundFont,
    result: &SoundFont,
    input_size: u64,
    predicted_size: u64,
) -> CliResult {
    let _ = font;
    let written = if args.dry_run {
        None
    } else {
        let output = args.output.as_ref().ok_or("missing --output")?;
        write_output(output, result)?;
        Some(output.display().to_string())
    };
    emit_json(&ExtractJson {
        kept: (0..result.presets.len())
            .map(|index| preset_json(result, index))
            .collect(),
        instruments: result.instruments.len(),
        samples: result.samples.len(),
        input_size,
        predicted_size,
        written,
    })?;
    Ok(ExitCode::SUCCESS)
}

/// Loads, validates, merges, re-validates, and writes several fonts.
fn cmd_merge(args: &MergeArgs) -> CliResult {
    let mut fonts = Vec::with_capacity(args.inputs.len());
    let mut input_size = 0;
    for input in &args.inputs {
        let (font, size) = load_font(input)?;
        let issues = validate(&font);
        if has_errors(&issues) {
            print_issues(&issues);
            return Err(format!("{} fails validation; aborting", input.display()).into());
        }
        if !args.json {
            print_issues(&issues);
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
        emit_json(&ExtractJson {
            kept: (0..result.presets.len())
                .map(|index| preset_json(&result, index))
                .collect(),
            instruments: result.instruments.len(),
            samples: result.samples.len(),
            input_size,
            predicted_size,
            written: Some(args.output.display().to_string()),
        })?;
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
