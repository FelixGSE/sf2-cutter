//! End-to-end extraction pipeline tests through the public API only:
//! build → write → parse → select → extract → validate → write → parse.
#![allow(clippy::unwrap_used)] // the entire file is test code

use sf2_cutter::builder::{SoundFontBuilder, instrument_zone, sample_zone};
use sf2_cutter::extract::{Options, extract};
use sf2_cutter::select::{Recipe, Selection};
use sf2_cutter::validate::{has_errors, validate};
use sf2_cutter::{parse, write};

/// A small GM-flavoured font: two pianos, one stereo string pad, one drum kit.
fn gm_like_font_bytes() -> Vec<u8> {
    let mut builder = SoundFontBuilder::new("GM-ish");
    let piano = builder
        .add_sample("piano", &vec![100; 300], 44100, 60)
        .unwrap();
    let epiano = builder
        .add_sample("epiano", &vec![-100; 250], 44100, 60)
        .unwrap();
    let left = builder
        .add_sample("str-l", &vec![7; 400], 44100, 57)
        .unwrap();
    let right = builder
        .add_sample("str-r", &vec![-7; 400], 44100, 57)
        .unwrap();
    builder.link_stereo(left, right).unwrap();
    let kick = builder
        .add_sample("kick", &vec![99; 120], 22050, 36)
        .unwrap();

    let piano_inst = builder
        .add_instrument("Piano I", vec![sample_zone(piano)])
        .unwrap();
    let epiano_inst = builder
        .add_instrument("E-Piano I", vec![sample_zone(epiano)])
        .unwrap();
    let strings_inst = builder
        .add_instrument("Strings I", vec![sample_zone(left), sample_zone(right)])
        .unwrap();
    let drums_inst = builder
        .add_instrument("Drums I", vec![sample_zone(kick)])
        .unwrap();

    builder.add_preset("Grand Piano", 0, 0, vec![instrument_zone(piano_inst)]);
    builder.add_preset("Electric Piano 1", 0, 4, vec![instrument_zone(epiano_inst)]);
    builder.add_preset(
        "String Ensemble",
        0,
        48,
        vec![instrument_zone(strings_inst)],
    );
    builder.add_preset("Standard Kit", 128, 0, vec![instrument_zone(drums_inst)]);
    write::write(&builder.build()).unwrap()
}

#[test]
fn pipeline_should_produce_piano_only_font_when_pattern_matches_pianos() {
    // given
    let input = gm_like_font_bytes();
    let font = parse::parse(&input).unwrap();
    let mut selection = Selection::new();
    selection.add_pattern("piano");

    // when
    let result = extract(&font, &selection, &Options::default()).unwrap();
    let output = write::write(&result).unwrap();
    let reparsed = parse::parse(&output).unwrap();

    // then
    let names: Vec<String> = reparsed
        .presets
        .iter()
        .map(|p| p.name.to_display())
        .collect();
    assert_eq!(names, vec!["Grand Piano", "Electric Piano 1"]);
    assert_eq!(reparsed.instruments.len(), 2);
    assert_eq!(reparsed.samples.len(), 2);
    assert!(!has_errors(&validate(&reparsed)));
    assert!(output.len() < input.len());
}

#[test]
fn pipeline_should_preserve_addresses_and_links_when_extracting_strings() {
    // given
    let font = parse::parse(&gm_like_font_bytes()).unwrap();
    let mut selection = Selection::new();
    selection.add_spec("0:48".parse().unwrap());

    // when
    let result = extract(&font, &selection, &Options::default()).unwrap();
    let reparsed = parse::parse(&write::write(&result).unwrap()).unwrap();

    // then: address preserved, stereo pair intact and mutually linked
    assert_eq!(reparsed.presets[0].bank, 0);
    assert_eq!(reparsed.presets[0].program, 48);
    assert_eq!(reparsed.samples.len(), 2);
    assert_eq!(reparsed.samples[0].sample_link, 1);
    assert_eq!(reparsed.samples[1].sample_link, 0);
    assert!(!has_errors(&validate(&reparsed)));
}

#[test]
fn pipeline_should_honour_recipe_when_combining_criteria() {
    // given: a recipe selecting pianos by pattern plus the drum bank
    let font = parse::parse(&gm_like_font_bytes()).unwrap();
    let recipe = Recipe::from_toml_str(
        r#"
        match = ["*grand*"]
        presets = ["0:4"]
        keep_drums = true
        renumber = true
        "#,
    )
    .unwrap();
    let mut selection = Selection::new();
    selection.apply_recipe(&recipe).unwrap();

    // when
    let result = extract(
        &font,
        &selection,
        &Options {
            renumber: recipe.renumber,
            ..Options::default()
        },
    )
    .unwrap();

    // then: three presets kept, programs compacted per bank
    let kept: Vec<(u16, u16, String)> = result
        .presets
        .iter()
        .map(|p| (p.bank, p.program, p.name.to_display()))
        .collect();
    assert_eq!(
        kept,
        vec![
            (0, 0, "Grand Piano".to_string()),
            (0, 1, "Electric Piano 1".to_string()),
            (128, 0, "Standard Kit".to_string()),
        ]
    );
}

#[test]
fn fluidsynth_should_load_extracted_synthetic_font_when_available() {
    // given: a piano-only font extracted from the synthetic GM bank
    if std::process::Command::new("fluidsynth")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: fluidsynth not on PATH");
        return;
    }
    let font = parse::parse(&gm_like_font_bytes()).unwrap();
    let mut selection = Selection::new();
    selection.add_pattern("piano");
    let result = extract(&font, &selection, &Options::default()).unwrap();
    let path = std::env::temp_dir().join("sf2-cutter-synthetic-smoke.sf2");
    std::fs::write(&path, write::write(&result).unwrap()).unwrap();

    // when: load headlessly (no midi in, file driver to /dev/null) and quit
    let output = std::process::Command::new("fluidsynth")
        .args(["-i", "-n", "-a", "file", "-F", "/dev/null"])
        .arg(&path)
        .output()
        .unwrap();

    // then: no load errors in the log. The exit code is useless here (255
    // either way, because no midi file is given), and warnings are expected
    // (e.g. "no preset found on channel 9" for fonts without a drum kit).
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_lowercase();
    assert!(
        !log.contains("error") && !log.contains("failed to load"),
        "fluidsynth rejected the font: {log}"
    );
}

#[test]
fn pipeline_should_fail_cleanly_when_nothing_matches() {
    // given
    let font = parse::parse(&gm_like_font_bytes()).unwrap();
    let mut selection = Selection::new();
    selection.add_pattern("sitar");

    // when
    let result = extract(&font, &selection, &Options::default());

    // then
    assert!(matches!(result, Err(sf2_cutter::Error::EmptySelection)));
}
