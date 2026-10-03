//! Tests against the real `FluidR3_GM.sf2` (141 MB, gitignored, not part of
//! the normal test run). Execute with:
//!
//! ```sh
//! cargo test -- --ignored
//! ```
//!
//! Each test skips gracefully when the file is absent.
#![allow(clippy::unwrap_used)] // the entire file is test code

use sf2_cutter::extract::{Options, extract};
use sf2_cutter::select::Selection;
use sf2_cutter::validate::{has_errors, validate};
use sf2_cutter::{parse, write};

const REAL_FONT: &str = "FluidR3_GM.sf2";

fn load_real_font() -> Option<Vec<u8>> {
    let bytes = std::fs::read(REAL_FONT).ok();
    if bytes.is_none() {
        eprintln!("skipped: {REAL_FONT} not present");
    }
    bytes
}

#[test]
#[ignore = "requires FluidR3_GM.sf2 in the crate root"]
fn parser_should_read_expected_counts_when_loading_fluid_r3() {
    // given
    let Some(bytes) = load_real_font() else {
        return;
    };

    // when
    let font = parse::parse(&bytes).unwrap();

    // then
    assert_eq!(font.presets.len(), 189);
    assert_eq!(font.instruments.len(), 193);
    assert_eq!(font.samples.len(), 1418);
    assert_eq!(font.version(), Some((2, 1)));
    assert_eq!(font.name().as_deref(), Some("Fluid R3 GM"));
    assert!(font.sample_data_24.is_none());
}

#[test]
#[ignore = "requires FluidR3_GM.sf2 in the crate root"]
fn writer_should_round_trip_structurally_when_reserialising_fluid_r3() {
    // given
    let Some(bytes) = load_real_font() else {
        return;
    };
    let font = parse::parse(&bytes).unwrap();

    // when
    let rewritten = write::write(&font).unwrap();
    let reparsed = parse::parse(&rewritten).unwrap();

    // then
    assert_eq!(reparsed, font);
}

#[test]
#[ignore = "requires FluidR3_GM.sf2 in the crate root"]
fn writer_should_reproduce_exact_bytes_when_reserialising_fluid_r3() {
    // given
    let Some(bytes) = load_real_font() else {
        return;
    };

    // when
    let rewritten = write::write(&parse::parse(&bytes).unwrap()).unwrap();

    // then: FluidR3 is canonical, so the round trip is byte-identical
    assert_eq!(rewritten.len(), bytes.len());
    assert!(
        rewritten == bytes,
        "round-tripped bytes differ from original"
    );
}

#[test]
#[ignore = "requires FluidR3_GM.sf2 in the crate root"]
fn validator_should_report_no_errors_when_checking_fluid_r3() {
    // given
    let Some(bytes) = load_real_font() else {
        return;
    };
    let font = parse::parse(&bytes).unwrap();

    // when
    let issues = validate(&font);

    // then
    assert!(!has_errors(&issues), "unexpected errors: {issues:?}");
}

#[test]
#[ignore = "requires FluidR3_GM.sf2 in the crate root"]
fn extractor_should_produce_small_valid_font_when_matching_piano() {
    // given
    let Some(bytes) = load_real_font() else {
        return;
    };
    let font = parse::parse(&bytes).unwrap();
    let mut selection = Selection::new();
    selection.add_pattern("piano");

    // when
    let result = extract(&font, &selection, &Options::default()).unwrap();
    let output = write::write(&result).unwrap();
    let reparsed = parse::parse(&output).unwrap();

    // then: every kept preset mentions piano, output is valid and far smaller
    assert_ne!(reparsed.presets.len(), 0);
    for preset in &reparsed.presets {
        assert!(
            preset.name.to_display().to_lowercase().contains("piano"),
            "unexpected preset kept: {}",
            preset.name
        );
    }
    assert!(!has_errors(&validate(&reparsed)));
    assert!(
        output.len() * 5 < bytes.len(),
        "expected at least 5x shrink, got {} -> {}",
        bytes.len(),
        output.len()
    );
}

#[test]
#[ignore = "requires MuseScore_General.sf3 in the crate root (and the sf3 feature)"]
#[cfg(feature = "sf3")]
fn extractor_should_decompress_sf3_when_matching_piano() {
    // given
    let Some(bytes) = std::fs::read("MuseScore_General.sf3").ok() else {
        eprintln!("skipped: MuseScore_General.sf3 not present");
        return;
    };
    let font = parse::parse(&bytes).unwrap();
    assert_eq!(font.version().map(|(major, _)| major), Some(3));
    let mut selection = Selection::new();
    selection.add_pattern("piano");

    // when
    let result = extract(&font, &selection, &Options::default()).unwrap();
    let output = write::write(&result).unwrap();
    let reparsed = parse::parse(&output).unwrap();

    // then: decoded to plain sf2 — version lowered, nothing compressed
    assert_eq!(reparsed.version(), Some((2, 4)));
    assert!(reparsed.samples.iter().all(|s| !s.is_compressed()));
    assert_ne!(reparsed.presets.len(), 0);
    assert!(!has_errors(&validate(&reparsed)));
}

#[test]
#[ignore = "requires fluidsynth on PATH and FluidR3_GM.sf2"]
fn fluidsynth_should_load_extracted_font_when_available() {
    // given
    let Some(bytes) = load_real_font() else {
        return;
    };
    if std::process::Command::new("fluidsynth")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: fluidsynth not on PATH");
        return;
    }
    let font = parse::parse(&bytes).unwrap();
    let mut selection = Selection::new();
    selection.add_pattern("piano");
    let result = extract(&font, &selection, &Options::default()).unwrap();
    let path = std::env::temp_dir().join("sf2-cutter-pianos-test.sf2");
    std::fs::write(&path, write::write(&result).unwrap()).unwrap();

    // when: load the font headlessly and quit immediately
    let status = std::process::Command::new("fluidsynth")
        .args(["-i", "-n", "-a", "file", "-F", "/dev/null"])
        .arg(&path)
        .output()
        .unwrap();

    // then: no load errors in the log. The exit code is useless here (255
    // either way, because no midi file is given), and warnings are expected.
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    )
    .to_lowercase();
    assert!(
        !log.contains("error") && !log.contains("failed to load"),
        "fluidsynth rejected the font: {log}"
    );
}
