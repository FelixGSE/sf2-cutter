//! Rebuild a minimal [`SoundFont`] containing only the selected presets.
//!
//! The reachability walk follows preset zones to instruments, instrument
//! zones to samples, and *mutual* sample links to their stereo partners
//! (one-directional links are treated as broken and sanitised). Everything
//! reachable is copied in original order with all indices remapped, and the
//! `smpl` data is re-sliced so only the kept samples' audio (plus the spec's
//! 46 zero guard points each) remains.

use std::collections::{BTreeMap, BTreeSet};

use crate::builder::GUARD_POINTS;
use crate::error::Error;
use crate::model::{
    GEN_INSTRUMENT, GEN_SAMPLE_ID, Instrument, Preset, SAMPLE_TYPE_ROM, SampleHeader, SoundFont,
    Zone,
};
use crate::select::Selection;

/// Extraction options.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Compact program numbers per bank, starting at 0, in original order.
    pub renumber: bool,
    /// Replace the output's `INAM` (bank name); `None` keeps the original.
    pub rename: Option<String>,
}

/// Extracts the presets matched by `selection` into a new font.
///
/// # Errors
///
/// Returns [`Error::EmptySelection`] when nothing matches,
/// [`Error::RenumberOverflow`] when renumbering would exceed program 127,
/// [`Error::TooLarge`] when the kept audio exceeds a 32-bit sample offset, or
/// [`Error::IndexOutOfBounds`] when the font contains dangling references
/// (run [`crate::validate::validate`] first for a full report).
pub fn extract(
    font: &SoundFont,
    selection: &Selection,
    options: &Options,
) -> Result<SoundFont, Error> {
    let kept_presets = selected_presets(font, selection)?;
    let kept_instruments = reachable_instruments(font, &kept_presets)?;
    let kept_samples = reachable_samples(font, &kept_instruments)?;

    let instrument_map = index_map(&kept_instruments)?;
    let sample_map = index_map(&kept_samples)?;

    let (samples, sample_data, sample_data_24) = rebuild_samples(font, &kept_samples, &sample_map)?;

    Ok(SoundFont {
        info: stamped_info(&font.info, options.rename.as_deref()),
        sample_data,
        sample_data_24,
        presets: rebuild_presets(font, &kept_presets, &instrument_map, options)?,
        instruments: rebuild_instruments(font, &kept_instruments, &sample_map)?,
        samples,
    })
}

/// Unique bytes of non-ROM sample data reachable from one preset; the basis
/// for the per-preset size estimate in listings. Dangling references are
/// ignored rather than reported.
#[must_use]
pub fn preset_sample_bytes(font: &SoundFont, preset_index: usize) -> u64 {
    let Some(preset) = font.presets.get(preset_index) else {
        return 0;
    };
    let mut samples = BTreeSet::new();
    for instrument_index in instrument_refs(preset) {
        let Some(instrument) = font.instruments.get(instrument_index) else {
            continue;
        };
        for sample_index in sample_refs(instrument) {
            collect_with_links(font, sample_index, &mut samples);
        }
    }
    samples
        .iter()
        .filter_map(|&index| font.samples.get(index))
        .filter(|sample| !sample.is_rom())
        .map(|sample| u64::from(sample.len_points()) * 2)
        .sum()
}

fn instrument_refs(preset: &Preset) -> impl Iterator<Item = usize> + '_ {
    preset
        .zones
        .iter()
        .filter_map(Zone::instrument_ref)
        .map(usize::from)
}

fn sample_refs(instrument: &Instrument) -> impl Iterator<Item = usize> + '_ {
    instrument
        .zones
        .iter()
        .filter_map(Zone::sample_ref)
        .map(usize::from)
}

/// Adds `index` and every sample reachable through link chains, ignoring
/// out-of-range links (strict extraction reports them separately).
fn collect_with_links(font: &SoundFont, index: usize, out: &mut BTreeSet<usize>) {
    let mut next = Some(index);
    while let Some(current) = next {
        if font.samples.get(current).is_none() {
            return;
        }
        if !out.insert(current) {
            return;
        }
        next = font.mutual_link(current);
    }
}

/// Clones the `INFO` chunks with provenance applied: optional `INAM`
/// replacement and the tool name appended to `ISFT` (the colon-separated
/// tool-chain convention, e.g. `SFEDT v1.28:SWAMI v0.9.4:sf2-cutter v0.1.0`).
pub(crate) fn stamped_info(
    info: &[crate::model::InfoChunk],
    rename: Option<&str>,
) -> Vec<crate::model::InfoChunk> {
    let mut out: Vec<crate::model::InfoChunk> = info.to_vec();
    if let Some(name) = rename {
        set_info_text(&mut out, *b"INAM", name);
    }
    let tool = concat!("sf2-cutter v", env!("CARGO_PKG_VERSION"));
    let existing = info
        .iter()
        .find(|c| c.id == *b"ISFT")
        .map(|c| trim_info_text(&c.data))
        .filter(|t| !t.is_empty());
    let chain = match existing {
        // ISFT is capped at 256 bytes including the terminator; fall back to
        // the tool name alone when appending would overflow.
        Some(prior) if prior.len() + 1 + tool.len() < 256 => format!("{prior}:{tool}"),
        _ => tool.to_string(),
    };
    set_info_text(&mut out, *b"ISFT", &chain);
    out
}

/// Text of an INFO chunk up to its first NUL.
fn trim_info_text(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

/// Replaces (or appends) an INFO chunk with NUL-terminated, even-length text.
fn set_info_text(info: &mut Vec<crate::model::InfoChunk>, id: [u8; 4], text: &str) {
    let mut data = text.as_bytes().to_vec();
    data.push(0);
    if data.len() % 2 == 1 {
        data.push(0);
    }
    if let Some(chunk) = info.iter_mut().find(|c| c.id == id) {
        chunk.data = data;
    } else {
        info.push(crate::model::InfoChunk { id, data });
    }
}

fn selected_presets(font: &SoundFont, selection: &Selection) -> Result<Vec<usize>, Error> {
    let kept: Vec<usize> = font
        .presets
        .iter()
        .enumerate()
        .filter(|(index, preset)| selection.matches(*index, preset))
        .map(|(index, _)| index)
        .collect();
    if kept.is_empty() {
        return Err(Error::EmptySelection);
    }
    Ok(kept)
}

fn reachable_instruments(
    font: &SoundFont,
    kept_presets: &[usize],
) -> Result<BTreeSet<usize>, Error> {
    let mut kept = BTreeSet::new();
    for &preset_index in kept_presets {
        for instrument_index in instrument_refs(&font.presets[preset_index]) {
            if instrument_index >= font.instruments.len() {
                return Err(Error::IndexOutOfBounds {
                    what: "preset instrument reference",
                    index: instrument_index,
                    max: font.instruments.len().saturating_sub(1),
                });
            }
            kept.insert(instrument_index);
        }
    }
    Ok(kept)
}

fn reachable_samples(
    font: &SoundFont,
    kept_instruments: &BTreeSet<usize>,
) -> Result<BTreeSet<usize>, Error> {
    let mut kept = BTreeSet::new();
    for &instrument_index in kept_instruments {
        for sample_index in sample_refs(&font.instruments[instrument_index]) {
            if sample_index >= font.samples.len() {
                return Err(Error::IndexOutOfBounds {
                    what: "instrument sample reference",
                    index: sample_index,
                    max: font.samples.len().saturating_sub(1),
                });
            }
            collect_with_links(font, sample_index, &mut kept);
        }
    }
    Ok(kept)
}

/// Maps old indices (ordered) to new contiguous `u16` indices. The inputs
/// come from `u16` generator amounts and sample links, so there are at most
/// 65536 entries and every new index fits; the error path is defensive.
fn index_map(kept: &BTreeSet<usize>) -> Result<BTreeMap<usize, u16>, Error> {
    kept.iter()
        .enumerate()
        .map(|(new, &old)| {
            u16::try_from(new)
                .map(|new| (old, new))
                .map_err(|_| Error::TooManyRecords("kept records"))
        })
        .collect()
}

fn remap_zones(
    zones: &[Zone],
    ref_oper: u16,
    map: &BTreeMap<usize, u16>,
    what: &'static str,
) -> Result<Vec<Zone>, Error> {
    zones
        .iter()
        .map(|zone| {
            // Duplicate reference generators: the reachability walk follows
            // only the last one per zone (the rule synths apply), so earlier
            // duplicates are dropped here to keep walk and remap consistent.
            let last_ref = zone.gens.iter().rposition(|g| g.oper == ref_oper);
            let gens = zone
                .gens
                .iter()
                .enumerate()
                .filter_map(|(position, generator)| {
                    if generator.oper != ref_oper {
                        return Some(Ok(*generator));
                    }
                    if Some(position) != last_ref {
                        return None;
                    }
                    Some(
                        map.get(&usize::from(generator.amount))
                            .map(|&new| crate::model::Generator {
                                oper: ref_oper,
                                amount: new,
                            })
                            .ok_or(Error::IndexOutOfBounds {
                                what,
                                index: usize::from(generator.amount),
                                max: map.len().saturating_sub(1),
                            }),
                    )
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(Zone {
                gens,
                mods: zone.mods.clone(),
            })
        })
        .collect()
}

fn rebuild_presets(
    font: &SoundFont,
    kept_presets: &[usize],
    instrument_map: &BTreeMap<usize, u16>,
    options: &Options,
) -> Result<Vec<Preset>, Error> {
    let mut next_program: BTreeMap<u16, u16> = BTreeMap::new();
    kept_presets
        .iter()
        .map(|&index| {
            let preset = &font.presets[index];
            let program = if options.renumber {
                let next = next_program.entry(preset.bank).or_insert(0);
                if *next > 127 {
                    return Err(Error::RenumberOverflow { bank: preset.bank });
                }
                let assigned = *next;
                *next += 1;
                assigned
            } else {
                preset.program
            };
            Ok(Preset {
                program,
                zones: remap_zones(
                    &preset.zones,
                    GEN_INSTRUMENT,
                    instrument_map,
                    "preset instrument reference",
                )?,
                ..preset.clone()
            })
        })
        .collect()
}

fn rebuild_instruments(
    font: &SoundFont,
    kept_instruments: &BTreeSet<usize>,
    sample_map: &BTreeMap<usize, u16>,
) -> Result<Vec<Instrument>, Error> {
    kept_instruments
        .iter()
        .map(|&index| {
            let instrument = &font.instruments[index];
            Ok(Instrument {
                name: instrument.name,
                zones: remap_zones(
                    &instrument.zones,
                    GEN_SAMPLE_ID,
                    sample_map,
                    "instrument sample reference",
                )?,
            })
        })
        .collect()
}

type RebuiltSamples = (Vec<SampleHeader>, Vec<u8>, Option<Vec<u8>>);

fn rebuild_samples(
    font: &SoundFont,
    kept_samples: &BTreeSet<usize>,
    sample_map: &BTreeMap<usize, u16>,
) -> Result<RebuiltSamples, Error> {
    let mut headers = Vec::with_capacity(kept_samples.len());
    let mut data = Vec::new();
    // Unusable sm24 data (version < 2.04 or size mismatch) must be ignored
    // per SF2.04 §6.3; the output simply drops the chunk.
    let mut data_24 = font.sm24_usable().then(Vec::new);
    for &index in kept_samples {
        let sample = &font.samples[index];
        let mut header = sample.clone();
        remap_or_sanitize_link(font, index, &mut header, sample_map)?;
        if !sample.is_rom() {
            relocate_sample(font, sample, &mut header, &mut data, data_24.as_mut())?;
        }
        headers.push(header);
    }
    Ok((headers, data, data_24))
}

/// Rewrites the stereo link for the output font: mutual links are remapped
/// to new indices; broken links (one-directional, self-referential, or
/// dangling) are sanitised to an unlinked mono header so the output is clean.
fn remap_or_sanitize_link(
    font: &SoundFont,
    old_index: usize,
    header: &mut SampleHeader,
    sample_map: &BTreeMap<usize, u16>,
) -> Result<(), Error> {
    if let Some(partner) = font.mutual_link(old_index) {
        header.sample_link = *sample_map.get(&partner).ok_or(Error::IndexOutOfBounds {
            what: "sample link",
            index: partner,
            max: sample_map.len().saturating_sub(1),
        })?;
    } else if header.is_linked() {
        header.sample_type = (header.sample_type & SAMPLE_TYPE_ROM) | 1;
        header.sample_link = 0;
    }
    Ok(())
}

/// Copies one sample's audio (plus guard points) to the new `smpl` buffer and
/// rewrites the header's offsets relative to its new position.
fn relocate_sample(
    font: &SoundFont,
    sample: &SampleHeader,
    header: &mut SampleHeader,
    data: &mut Vec<u8>,
    data_24: Option<&mut Vec<u8>>,
) -> Result<(), Error> {
    let start = sample.start as usize;
    let end = sample.end as usize;
    let pcm = font
        .sample_data
        .get(start * 2..end * 2)
        .ok_or(Error::IndexOutOfBounds {
            what: "sample data range",
            index: end,
            max: font.sample_points(),
        })?;
    let new_start = u32::try_from(data.len() / 2).map_err(|_| Error::TooLarge("smpl"))?;
    data.extend_from_slice(pcm);
    data.extend_from_slice(&[0; GUARD_POINTS * 2]);
    if let Some(data_24) = data_24 {
        let lsb = font
            .sample_data_24
            .as_ref()
            .and_then(|d| d.get(start..end))
            .ok_or(Error::IndexOutOfBounds {
                what: "sm24 data range",
                index: end,
                max: font.sample_data_24.as_ref().map_or(0, Vec::len),
            })?;
        data_24.extend_from_slice(lsb);
        data_24.extend_from_slice(&[0; GUARD_POINTS]);
    }
    header.start = new_start;
    header.end = new_start
        .checked_add(sample.len_points())
        .ok_or(Error::TooLarge("smpl"))?;
    header.start_loop = shift_point(sample.start_loop, sample.start, new_start);
    header.end_loop = shift_point(sample.end_loop, sample.start, new_start);
    Ok(())
}

/// Moves a loop point by the sample's relocation offset, clamping at zero in
/// case the original point sat (invalidly) before the sample start.
fn shift_point(point: u32, old_start: u32, new_start: u32) -> u32 {
    let shifted = i64::from(point) - i64::from(old_start) + i64::from(new_start);
    u32::try_from(shifted.max(0)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{SoundFontBuilder, instrument_zone, sample_zone, test_font};
    use crate::validate::{has_errors, validate};

    fn renumber_options() -> Options {
        Options {
            renumber: true,
            ..Options::default()
        }
    }

    fn select_pattern(pattern: &str) -> Selection {
        let mut selection = Selection::new();
        selection.add_pattern(pattern);
        selection
    }

    #[test]
    fn extract_should_keep_only_piano_when_pattern_matches_one_preset() {
        // given
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("piano"), &Options::default()).unwrap();

        // then
        assert_eq!(result.presets.len(), 1);
        assert_eq!(result.presets[0].name.to_display(), "Bright Piano");
        assert_eq!(result.instruments.len(), 1);
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.sample_data.len(), (200 + GUARD_POINTS) * 2);
        assert!(!has_errors(&validate(&result)));
    }

    #[test]
    fn extract_should_preserve_bank_and_program_when_renumber_is_off() {
        // given
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("strings"), &Options::default()).unwrap();

        // then
        assert_eq!(result.presets[0].bank, 0);
        assert_eq!(result.presets[0].program, 48);
    }

    #[test]
    fn extract_should_remap_references_when_leading_records_are_dropped() {
        // given: "Slow Strings" uses instrument 1 and samples 1..=2
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("strings"), &Options::default()).unwrap();

        // then
        assert_eq!(result.presets[0].zones[0].instrument_ref(), Some(0));
        let instrument = &result.instruments[0];
        assert_eq!(instrument.zones[1].sample_ref(), Some(0));
        assert_eq!(instrument.zones[2].sample_ref(), Some(1));
        assert_eq!(result.samples[0].sample_link, 1);
        assert_eq!(result.samples[1].sample_link, 0);
        assert!(!has_errors(&validate(&result)));
    }

    #[test]
    fn extract_should_include_linked_sample_when_only_one_side_is_referenced() {
        // given: an instrument referencing only the left half of a stereo pair
        let mut builder = SoundFontBuilder::new("linked");
        let left = builder.add_sample("left", &[1; 100], 44100, 60).unwrap();
        let right = builder.add_sample("right", &[2; 100], 44100, 60).unwrap();
        builder.link_stereo(left, right).unwrap();
        let instrument = builder
            .add_instrument("Only Left", vec![sample_zone(left)])
            .unwrap();
        builder.add_preset("Lonely", 0, 0, vec![instrument_zone(instrument)]);
        let font = builder.build();

        // when
        let result = extract(&font, &select_pattern("lonely"), &Options::default()).unwrap();

        // then
        assert_eq!(result.samples.len(), 2);
        assert_eq!(result.samples[0].sample_link, 1);
        assert_eq!(result.samples[1].sample_link, 0);
        assert!(!has_errors(&validate(&result)));
    }

    #[test]
    fn extract_should_relocate_sample_offsets_when_preceding_samples_dropped() {
        // given: strings samples originally sit after the 200-point piano sample
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("strings"), &Options::default()).unwrap();

        // then
        let left = &result.samples[0];
        assert_eq!(left.start, 0);
        assert_eq!(left.end, 300);
        assert_eq!(left.start_loop, 0);
        assert_eq!(left.end_loop, 300);
        let right = &result.samples[1];
        let expected = u32::try_from(300 + GUARD_POINTS).unwrap();
        assert_eq!(right.start, expected);
        assert_eq!(right.end, expected + 300);
        assert_eq!(right.start_loop, expected);
        assert_eq!(right.end_loop, expected + 300);
    }

    #[test]
    fn extract_should_copy_sample_audio_when_slicing_smpl() {
        // given
        let font = test_font();
        let original = &font.samples[0];
        let expected_pcm =
            font.sample_data[original.start as usize * 2..original.end as usize * 2].to_vec();

        // when
        let result = extract(&font, &select_pattern("piano"), &Options::default()).unwrap();

        // then
        assert_eq!(&result.sample_data[..expected_pcm.len()], &expected_pcm[..]);
        assert!(
            result.sample_data[expected_pcm.len()..]
                .iter()
                .all(|&b| b == 0)
        );
    }

    #[test]
    fn extract_should_fail_when_selection_matches_nothing() {
        // given
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("accordion"), &Options::default());

        // then
        assert!(matches!(result, Err(Error::EmptySelection)));
    }

    #[test]
    fn extract_should_renumber_programs_per_bank_when_option_set() {
        // given
        let font = test_font();
        let mut selection = Selection::new();
        selection.add_pattern("strings");
        selection.set_keep_drums(true);

        // when
        let result = extract(&font, &selection, &renumber_options()).unwrap();

        // then: strings (bank 0) and drums (bank 128) each start at program 0
        assert_eq!(result.presets.len(), 2);
        assert_eq!(result.presets[0].bank, 0);
        assert_eq!(result.presets[0].program, 0);
        assert_eq!(result.presets[1].bank, 128);
        assert_eq!(result.presets[1].program, 0);
    }

    #[test]
    fn extract_should_keep_rom_header_offsets_when_sample_lives_in_rom() {
        // given: a preset playing a ROM sample
        let mut builder = SoundFontBuilder::new("rom");
        let sample = builder.add_sample("rom-sample", &[], 44100, 60).unwrap();
        let instrument = builder
            .add_instrument("Rom Inst", vec![sample_zone(sample)])
            .unwrap();
        builder.add_preset("Rom Preset", 0, 0, vec![instrument_zone(instrument)]);
        let mut font = builder.build();
        font.samples[0].sample_type = crate::model::SAMPLE_TYPE_ROM | 1;
        font.samples[0].start = 5000;
        font.samples[0].end = 6000;
        font.sample_data.clear();

        // when
        let result = extract(&font, &select_pattern("rom"), &Options::default()).unwrap();

        // then: ROM offsets survive untouched and no audio is copied
        assert_eq!(result.samples[0].start, 5000);
        assert_eq!(result.samples[0].end, 6000);
        assert_eq!(result.sample_data, Vec::<u8>::new());
    }

    #[test]
    fn extract_should_fail_when_preset_references_missing_instrument() {
        // given
        let mut font = test_font();
        font.presets[0].zones[0].gens[0].amount = 999;

        // when
        let result = extract(&font, &select_pattern("piano"), &Options::default());

        // then
        assert!(matches!(
            result,
            Err(Error::IndexOutOfBounds {
                what: "preset instrument reference",
                ..
            })
        ));
    }

    #[test]
    fn preset_sample_bytes_should_sum_unique_samples_when_preset_is_valid() {
        // given
        let font = test_font();

        // when / then: piano = 200 points, strings = 2 x 300 points
        assert_eq!(preset_sample_bytes(&font, 0), 200 * 2);
        assert_eq!(preset_sample_bytes(&font, 1), 600 * 2);
        assert_eq!(preset_sample_bytes(&font, 99), 0);
    }

    #[test]
    fn extract_should_sanitize_link_when_partner_does_not_link_back() {
        // given: a "left"-typed sample whose link target never links back
        let mut builder = SoundFontBuilder::new("broken");
        let broken = builder
            .add_sample("broken-left", &[5; 100], 44100, 60)
            .unwrap();
        let unrelated = builder
            .add_sample("unrelated", &[6; 100], 44100, 60)
            .unwrap();
        let instrument = builder
            .add_instrument("Broken", vec![sample_zone(broken)])
            .unwrap();
        builder.add_preset("Broken Link", 0, 0, vec![instrument_zone(instrument)]);
        let mut font = builder.build();
        font.samples[usize::from(broken)].sample_type = 4;
        font.samples[usize::from(broken)].sample_link = unrelated;

        // when
        let result = extract(&font, &select_pattern("broken"), &Options::default()).unwrap();

        // then: the bogus target is not dragged in and the header is mono
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.samples[0].sample_type, 1);
        assert_eq!(result.samples[0].sample_link, 0);
        assert_eq!(validate(&result), vec![]);
    }

    #[test]
    fn extract_should_sanitize_link_when_sample_links_to_itself() {
        // given
        let mut builder = SoundFontBuilder::new("selfie");
        let sample = builder.add_sample("self", &[7; 100], 44100, 60).unwrap();
        let instrument = builder
            .add_instrument("Selfie", vec![sample_zone(sample)])
            .unwrap();
        builder.add_preset("Selfie", 0, 0, vec![instrument_zone(instrument)]);
        let mut font = builder.build();
        font.samples[usize::from(sample)].sample_type = 4;
        font.samples[usize::from(sample)].sample_link = sample;

        // when
        let result = extract(&font, &select_pattern("selfie"), &Options::default()).unwrap();

        // then
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.samples[0].sample_type, 1);
        assert_eq!(result.samples[0].sample_link, 0);
    }

    #[test]
    fn extract_should_keep_last_instrument_when_zone_has_duplicate_instrument_generators() {
        // given: one preset zone carrying two instrument generators
        let mut builder = SoundFontBuilder::new("dup");
        let sample_a = builder.add_sample("a", &[1; 100], 44100, 60).unwrap();
        let sample_b = builder.add_sample("b", &[2; 100], 44100, 60).unwrap();
        let inst_a = builder
            .add_instrument("Inst A", vec![sample_zone(sample_a)])
            .unwrap();
        let inst_b = builder
            .add_instrument("Inst B", vec![sample_zone(sample_b)])
            .unwrap();
        builder.add_preset(
            "Dup",
            0,
            0,
            vec![Zone {
                gens: vec![
                    crate::model::Generator {
                        oper: crate::model::GEN_INSTRUMENT,
                        amount: inst_a,
                    },
                    crate::model::Generator {
                        oper: crate::model::GEN_INSTRUMENT,
                        amount: inst_b,
                    },
                ],
                mods: vec![],
            }],
        );
        let font = builder.build();

        // when
        let result = extract(&font, &select_pattern("dup"), &Options::default()).unwrap();

        // then: only the last reference survives, remapped to the kept instrument
        assert_eq!(result.instruments.len(), 1);
        assert_eq!(result.instruments[0].name.to_display(), "Inst B");
        assert_eq!(
            result.presets[0].zones[0].gens,
            vec![crate::model::Generator {
                oper: crate::model::GEN_INSTRUMENT,
                amount: 0,
            }]
        );
        assert!(!has_errors(&validate(&result)));
    }

    #[test]
    fn extract_should_renumber_through_program_127_when_bank_has_exactly_128_presets() {
        // given
        let mut builder = SoundFontBuilder::new("full-bank");
        for i in 0..128 {
            builder.add_preset(&format!("P{i}"), 0, i, vec![]);
        }
        let font = builder.build();

        // when
        let result = extract(&font, &select_pattern("p"), &renumber_options()).unwrap();

        // then: exactly 128 programs, 0 through 127
        assert_eq!(result.presets.len(), 128);
        assert_eq!(result.presets[0].program, 0);
        assert_eq!(result.presets[127].program, 127);
    }

    #[test]
    fn extract_should_fail_when_renumbered_bank_exceeds_128_programs() {
        // given: 129 presets on one bank
        let mut builder = SoundFontBuilder::new("crowded");
        for i in 0..129 {
            builder.add_preset(&format!("P{i}"), 0, i, vec![]);
        }
        let font = builder.build();

        // when
        let result = extract(&font, &select_pattern("p"), &renumber_options());

        // then
        assert!(matches!(result, Err(Error::RenumberOverflow { bank: 0 })));
    }

    #[test]
    fn extract_should_drop_sm24_when_data_is_unusable() {
        // given: an sm24 chunk whose size does not match the sample data
        let mut builder = SoundFontBuilder::new("bad24");
        let sample = builder.add_sample("s", &[1; 100], 44100, 60).unwrap();
        let instrument = builder
            .add_instrument("I", vec![sample_zone(sample)])
            .unwrap();
        builder.add_preset("Bad24", 0, 0, vec![instrument_zone(instrument)]);
        let mut font = builder.build();
        font.sample_data_24 = Some(vec![0; 5]);

        // when
        let result = extract(&font, &select_pattern("bad24"), &Options::default()).unwrap();

        // then: extraction succeeds and the bogus chunk is gone
        assert!(result.sample_data_24.is_none());
        assert_eq!(result.samples.len(), 1);
    }

    #[test]
    fn extract_should_stamp_isft_when_output_is_built() {
        // given: a font without any ISFT chunk
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("piano"), &Options::default()).unwrap();

        // then
        let isft = result.info_chunk(*b"ISFT").unwrap();
        let text = String::from_utf8_lossy(isft);
        assert!(text.contains("sf2-cutter v"));
    }

    #[test]
    fn extract_should_append_to_isft_chain_when_chunk_exists() {
        // given: a font with a prior tool chain
        let mut font = test_font();
        font.info.push(crate::model::InfoChunk {
            id: *b"ISFT",
            data: b"SFEDT v1.28\0".to_vec(),
        });

        // when
        let result = extract(&font, &select_pattern("piano"), &Options::default()).unwrap();

        // then
        let text = String::from_utf8_lossy(result.info_chunk(*b"ISFT").unwrap()).to_string();
        assert!(text.starts_with("SFEDT v1.28:sf2-cutter v"));
    }

    #[test]
    fn extract_should_replace_bank_name_when_rename_option_set() {
        // given
        let font = test_font();
        let options = Options {
            rename: Some("My Pianos".into()),
            ..Options::default()
        };

        // when
        let result = extract(&font, &select_pattern("piano"), &options).unwrap();

        // then
        assert_eq!(result.name().as_deref(), Some("My Pianos"));
    }

    #[test]
    fn extract_should_keep_bank_name_when_rename_option_absent() {
        // given
        let font = test_font();

        // when
        let result = extract(&font, &select_pattern("piano"), &Options::default()).unwrap();

        // then
        assert_eq!(result.name(), font.name());
    }

    #[test]
    fn extract_should_slice_sm24_when_present() {
        // given: a font with 24-bit extension data
        let mut builder = SoundFontBuilder::new("24bit");
        let a = builder.add_sample("a", &[10; 50], 44100, 60).unwrap();
        let b = builder.add_sample("b", &[20; 60], 44100, 60).unwrap();
        let inst_b = builder
            .add_instrument("B Inst", vec![sample_zone(b)])
            .unwrap();
        builder.add_preset("Keep B", 0, 0, vec![instrument_zone(inst_b)]);
        let _ = a;
        let mut font = builder.build();
        let points = font.sample_points();
        font.sample_data_24 = Some(
            (0..points)
                .map(|i| u8::try_from(i % 251).unwrap())
                .collect(),
        );
        let b_header = font.samples[1].clone();
        let expected: Vec<u8> = (b_header.start as usize..b_header.end as usize)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();

        // when
        let result = extract(&font, &select_pattern("keep"), &Options::default()).unwrap();

        // then
        let sliced = result.sample_data_24.unwrap();
        assert_eq!(&sliced[..expected.len()], &expected[..]);
        assert_eq!(sliced.len(), 60 + GUARD_POINTS);
    }
}
