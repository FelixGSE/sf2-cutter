//! Property-based tests: random (always-valid) fonts generated through the
//! public builder, checked against the crate's core invariants.
//!
//! Case count honours `PROPTEST_CASES`; `make mutants` sets it to 4 so
//! mutation runs stay fast.
#![allow(clippy::unwrap_used)] // the entire file is test code

use proptest::prelude::*;
use sf2_cutter::builder::{SoundFontBuilder, instrument_zone, sample_zone};
use sf2_cutter::extract::{Options, extract};
use sf2_cutter::model::SoundFont;
use sf2_cutter::select::Selection;
use sf2_cutter::validate::{has_errors, validate};
use sf2_cutter::{parse, write};

/// Plain-data description of a random font, mapped to a [`SoundFont`] by
/// [`build_font`]. Index fields are reduced modulo the available range, so
/// every spec builds a structurally valid font.
#[derive(Debug, Clone)]
struct FontSpec {
    samples: Vec<Vec<i16>>,
    stereo_pair: bool,
    instruments: Vec<Vec<usize>>,
    presets: Vec<(u16, u16, Vec<usize>)>,
}

fn arb_font_spec() -> impl Strategy<Value = FontSpec> {
    let samples = prop::collection::vec(prop::collection::vec(any::<i16>(), 48..200), 1..6usize);
    let instruments =
        prop::collection::vec(prop::collection::vec(any::<usize>(), 1..4usize), 1..4usize);
    let presets = prop::collection::vec(
        (
            0u16..=128,
            0u16..=127,
            prop::collection::vec(any::<usize>(), 1..3usize),
        ),
        1..4usize,
    );
    (samples, any::<bool>(), instruments, presets).prop_map(
        |(samples, stereo_pair, instruments, presets)| FontSpec {
            samples,
            stereo_pair,
            instruments,
            presets,
        },
    )
}

fn build_font(spec: &FontSpec) -> SoundFont {
    let mut builder = SoundFontBuilder::new("Property Font");
    let mut sample_ids = Vec::new();
    for (index, pcm) in spec.samples.iter().enumerate() {
        sample_ids.push(
            builder
                .add_sample(&format!("s{index}"), pcm, 44_100, 60)
                .unwrap(),
        );
    }
    if spec.stereo_pair && sample_ids.len() >= 2 {
        builder.link_stereo(sample_ids[0], sample_ids[1]).unwrap();
    }
    let mut instrument_ids = Vec::new();
    for (index, zones) in spec.instruments.iter().enumerate() {
        let zones = zones
            .iter()
            .map(|pick| sample_zone(sample_ids[pick % sample_ids.len()]))
            .collect();
        instrument_ids.push(builder.add_instrument(&format!("i{index}"), zones).unwrap());
    }
    for (index, (bank, program, picks)) in spec.presets.iter().enumerate() {
        let zones = picks
            .iter()
            .map(|pick| instrument_zone(instrument_ids[pick % instrument_ids.len()]))
            .collect();
        builder.add_preset(&format!("p{index}"), *bank, *program, zones);
    }
    builder.build()
}

proptest! {
    #[test]
    fn writer_should_round_trip_model_when_font_is_arbitrary(spec in arb_font_spec()) {
        // given
        let font = build_font(&spec);

        // when
        let bytes = write::write(&font).unwrap();
        let reparsed = parse::parse(&bytes).unwrap();

        // then
        prop_assert_eq!(&reparsed, &font);
    }

    #[test]
    fn writer_should_be_idempotent_when_reserialising_arbitrary_font(spec in arb_font_spec()) {
        // given
        let bytes = write::write(&build_font(&spec)).unwrap();

        // when
        let again = write::write(&parse::parse(&bytes).unwrap()).unwrap();

        // then
        prop_assert_eq!(again, bytes);
    }

    #[test]
    fn file_size_should_predict_written_length_when_font_is_arbitrary(spec in arb_font_spec()) {
        // given
        let font = build_font(&spec);

        // when
        let bytes = write::write(&font).unwrap();

        // then
        prop_assert_eq!(write::file_size(&font), bytes.len() as u64);
    }

    #[test]
    fn extractor_should_produce_valid_font_when_matching_everything(spec in arb_font_spec()) {
        // given: every generated preset name starts with "p"
        let font = build_font(&spec);
        let mut selection = Selection::new();
        selection.add_pattern("p");

        // when
        let result = extract(&font, &selection, &Options::default()).unwrap();

        // then: output validates (modulo warnings) and round-trips
        prop_assert!(!has_errors(&validate(&result)));
        prop_assert_eq!(result.presets.len(), font.presets.len());
        let reparsed = parse::parse(&write::write(&result).unwrap()).unwrap();
        prop_assert_eq!(&reparsed, &result);
    }

    #[test]
    fn extractor_should_keep_all_references_in_bounds_when_extracting_single_preset(
        spec in arb_font_spec(),
        pick in any::<usize>(),
    ) {
        // given
        let font = build_font(&spec);
        let mut selection = Selection::new();
        selection.add_index(pick % font.presets.len());

        // when
        let result = extract(&font, &selection, &Options::default()).unwrap();

        // then: every reference in the shrunken font stays in bounds
        prop_assert_eq!(result.presets.len(), 1);
        for preset in &result.presets {
            for zone in &preset.zones {
                if let Some(instrument) = zone.instrument_ref() {
                    prop_assert!(usize::from(instrument) < result.instruments.len());
                }
            }
        }
        for instrument in &result.instruments {
            for zone in &instrument.zones {
                if let Some(sample) = zone.sample_ref() {
                    prop_assert!(usize::from(sample) < result.samples.len());
                }
            }
        }
        for sample in &result.samples {
            prop_assert!(u64::from(sample.end) * 2 <= result.sample_data.len() as u64);
        }
    }
}
