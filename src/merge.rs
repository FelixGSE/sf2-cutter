//! Combine several [`SoundFont`]s into one.
//!
//! Presets, instruments, and samples are concatenated in input order with
//! every cross-reference re-indexed, and the sample data (including 24-bit
//! extensions) is spliced together.

use crate::error::Error;
use crate::model::{
    GEN_INSTRUMENT, GEN_SAMPLE_ID, Instrument, Preset, SampleHeader, SoundFont, Zone,
};

/// Merges fonts in order.
///
/// `INFO` comes from the first font (plus provenance stamping and the
/// optional `rename` of the bank name, exactly like extraction). Duplicate
/// `bank:program` addresses are kept as-is — resolve them afterwards with the
/// remapping options of `extract`.
///
/// When any input carries usable 24-bit data, the output gets an `sm24` chunk
/// with zero LSBs for the inputs that lack one, and the output version is
/// raised to 2.04 if needed; otherwise `sm24` data is dropped.
///
/// # Errors
///
/// Returns [`Error::NothingToMerge`] for an empty input list,
/// [`Error::TooManyRecords`] when combined instruments or samples exceed
/// 16-bit SF2 indices, or [`Error::TooLarge`] when combined sample data
/// exceeds 32-bit sample offsets.
pub fn merge(fonts: &[SoundFont], rename: Option<&str>) -> Result<SoundFont, Error> {
    let [first, ..] = fonts else {
        return Err(Error::NothingToMerge);
    };
    let with_24 = fonts.iter().any(SoundFont::sm24_usable);
    let mut out = SoundFont {
        info: crate::extract::stamped_info(&first.info, rename),
        sample_data_24: with_24.then(Vec::new),
        ..SoundFont::default()
    };
    if with_24 {
        ensure_version_2_04(&mut out.info);
    }

    for font in fonts {
        let instrument_offset = u16::try_from(out.instruments.len())
            .map_err(|_| Error::TooManyRecords("instruments"))?;
        let sample_offset =
            u16::try_from(out.samples.len()).map_err(|_| Error::TooManyRecords("samples"))?;
        let point_offset =
            u32::try_from(out.sample_data.len() / 2).map_err(|_| Error::TooLarge("smpl"))?;
        let byte_offset =
            u32::try_from(out.sample_data.len()).map_err(|_| Error::TooLarge("smpl"))?;

        for preset in &font.presets {
            out.presets.push(Preset {
                zones: shift_zones(&preset.zones, GEN_INSTRUMENT, instrument_offset)?,
                ..preset.clone()
            });
        }
        for instrument in &font.instruments {
            out.instruments.push(Instrument {
                name: instrument.name,
                zones: shift_zones(&instrument.zones, GEN_SAMPLE_ID, sample_offset)?,
            });
        }
        for sample in &font.samples {
            out.samples.push(shift_sample(
                sample,
                sample_offset,
                point_offset,
                byte_offset,
            )?);
        }
        out.sample_data.extend_from_slice(&font.sample_data);
        // Keep the next font's point offsets exact (and sm24 aligned) even if
        // this input's smpl had an odd byte length, as raw SF3 streams can.
        if out.sample_data.len() % 2 == 1 {
            out.sample_data.push(0);
            if let Some(data_24) = &mut out.sample_data_24 {
                data_24.push(0);
            }
        }
        append_sm24(&mut out, font);
        ensure_addressable(out.instruments.len(), "instruments")?;
        ensure_addressable(out.samples.len(), "samples")?;
    }
    Ok(out)
}

/// 16-bit references address at most 65536 records.
fn ensure_addressable(count: usize, what: &'static str) -> Result<(), Error> {
    if count > usize::from(u16::MAX) + 1 {
        return Err(Error::TooManyRecords(what));
    }
    Ok(())
}

/// Raises `ifil` to 2.04 when it is older or malformed (required for `sm24`
/// to be used).
fn ensure_version_2_04(info: &mut [crate::model::InfoChunk]) {
    if let Some(chunk) = info.iter_mut().find(|c| c.id == *b"ifil") {
        let old = (chunk.data.len() >= 4).then(|| {
            (
                u16::from_le_bytes([chunk.data[0], chunk.data[1]]),
                u16::from_le_bytes([chunk.data[2], chunk.data[3]]),
            )
        });
        if old.is_none_or(|version| version < (2, 4)) {
            chunk.data = vec![2, 0, 4, 0];
        }
    }
}

/// Clones zones with every reference generator shifted by `offset`.
fn shift_zones(zones: &[Zone], ref_oper: u16, offset: u16) -> Result<Vec<Zone>, Error> {
    zones
        .iter()
        .map(|zone| {
            let gens = zone
                .gens
                .iter()
                .map(|generator| {
                    if generator.oper != ref_oper {
                        return Ok(*generator);
                    }
                    generator
                        .amount
                        .checked_add(offset)
                        .map(|amount| crate::model::Generator {
                            oper: ref_oper,
                            amount,
                        })
                        .ok_or(Error::TooManyRecords("merged references"))
                })
                .collect::<Result<Vec<_>, Error>>()?;
            Ok(Zone {
                gens,
                mods: zone.mods.clone(),
            })
        })
        .collect()
}

/// Clones a sample header with its link and (for RAM samples) its data
/// offsets shifted into the merged address space. SF3-compressed samples use
/// BYTE offsets for their Ogg stream and decoded-relative loop points, so
/// they shift by bytes and keep their loops.
fn shift_sample(
    sample: &SampleHeader,
    sample_offset: u16,
    point_offset: u32,
    byte_offset: u32,
) -> Result<SampleHeader, Error> {
    let mut header = sample.clone();
    if header.is_linked() {
        header.sample_link = header
            .sample_link
            .checked_add(sample_offset)
            .ok_or(Error::TooManyRecords("merged sample links"))?;
    }
    if header.is_rom() {
        return Ok(header);
    }
    if header.is_compressed() {
        let shift = |byte: u32| byte.checked_add(byte_offset).ok_or(Error::TooLarge("smpl"));
        header.start = shift(header.start)?;
        header.end = shift(header.end)?;
    } else {
        let shift = |point: u32| {
            point
                .checked_add(point_offset)
                .ok_or(Error::TooLarge("smpl"))
        };
        header.start = shift(header.start)?;
        header.end = shift(header.end)?;
        header.start_loop = shift(header.start_loop)?;
        header.end_loop = shift(header.end_loop)?;
    }
    Ok(header)
}

/// Appends one font's 24-bit LSBs (or zeros) so `sm24` stays one byte per
/// point of the concatenated `smpl`. Pad bytes of odd-point inputs are
/// stripped here; the writer re-pads the final chunk.
fn append_sm24(out: &mut SoundFont, font: &SoundFont) {
    let points = font.sample_points();
    if let Some(data_24) = &mut out.sample_data_24 {
        if font.sm24_usable()
            && let Some(lsb) = &font.sample_data_24
        {
            data_24.extend_from_slice(&lsb[..points]);
            return;
        }
        data_24.extend(std::iter::repeat_n(0, points));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{SoundFontBuilder, instrument_zone, sample_zone};
    use crate::validate::{has_errors, validate};

    fn simple_font(tag: &str, bank: u16, program: u16, pcm_len: usize) -> SoundFont {
        let mut builder = SoundFontBuilder::new(tag);
        let sample = builder
            .add_sample(&format!("{tag}-s"), &vec![7; pcm_len], 44_100, 60)
            .unwrap();
        let instrument = builder
            .add_instrument(&format!("{tag}-i"), vec![sample_zone(sample)])
            .unwrap();
        builder.add_preset(tag, bank, program, vec![instrument_zone(instrument)]);
        builder.build()
    }

    #[test]
    fn merge_should_concatenate_and_remap_when_merging_two_fonts() {
        // given
        let first = simple_font("alpha", 0, 0, 100);
        let second = simple_font("beta", 0, 1, 150);

        // when
        let merged = merge(&[first.clone(), second], None).unwrap();

        // then: counts add up and the second font's references are shifted
        assert_eq!(merged.presets.len(), 2);
        assert_eq!(merged.instruments.len(), 2);
        assert_eq!(merged.samples.len(), 2);
        assert_eq!(merged.presets[1].zones[0].instrument_ref(), Some(1));
        assert_eq!(merged.instruments[1].zones[0].sample_ref(), Some(1));
        let first_points = u32::try_from(first.sample_points()).unwrap();
        assert_eq!(merged.samples[1].start, first_points);
        assert!(!has_errors(&validate(&merged)));
    }

    #[test]
    fn merge_should_round_trip_when_written_and_reparsed() {
        // given
        let merged = merge(
            &[simple_font("a", 0, 0, 80), simple_font("b", 1, 2, 90)],
            None,
        )
        .unwrap();

        // when
        let reparsed = crate::parse::parse(&crate::write::write(&merged).unwrap()).unwrap();

        // then
        assert_eq!(reparsed, merged);
    }

    #[test]
    fn merge_should_shift_sample_links_when_second_font_has_stereo_pair() {
        // given
        let first = simple_font("mono", 0, 0, 100);
        let mut builder = SoundFontBuilder::new("stereo");
        let left = builder.add_sample("l", &[1; 100], 44_100, 60).unwrap();
        let right = builder.add_sample("r", &[2; 100], 44_100, 60).unwrap();
        builder.link_stereo(left, right).unwrap();
        let instrument = builder
            .add_instrument("st-i", vec![sample_zone(left), sample_zone(right)])
            .unwrap();
        builder.add_preset("stereo", 0, 1, vec![instrument_zone(instrument)]);
        let second = builder.build();

        // when
        let merged = merge(&[first, second], None).unwrap();

        // then: pair sits at indices 1 and 2, still mutually linked
        assert_eq!(merged.samples[1].sample_link, 2);
        assert_eq!(merged.samples[2].sample_link, 1);
        assert_eq!(merged.mutual_link(1), Some(2));
        assert!(!has_errors(&validate(&merged)));
    }

    #[test]
    fn merge_should_zero_fill_sm24_when_only_one_input_has_it() {
        // given: the second font carries usable 24-bit data
        let first = simple_font("plain", 0, 0, 100);
        let mut second = simple_font("deep", 0, 1, 50);
        let points = second.sample_points();
        second.sample_data_24 = Some(
            (0..points)
                .map(|i| u8::try_from(i % 200).unwrap())
                .collect(),
        );
        assert!(second.sm24_usable());

        // when
        let merged = merge(&[first.clone(), second.clone()], None).unwrap();

        // then: zeros for the first font, real LSBs for the second
        let sm24 = merged.sample_data_24.as_ref().unwrap();
        assert_eq!(sm24.len(), merged.sample_points());
        let first_points = first.sample_points();
        assert!(sm24[..first_points].iter().all(|&b| b == 0));
        assert_eq!(&sm24[first_points..], &second.sample_data_24.unwrap()[..]);
        assert!(merged.sm24_usable());
    }

    #[test]
    fn merge_should_bump_version_when_sm24_present_and_first_font_is_old() {
        // given: first font declares 2.01, second carries sm24
        let mut first = simple_font("old", 0, 0, 100);
        if let Some(chunk) = first.info.iter_mut().find(|c| c.id == *b"ifil") {
            chunk.data = vec![2, 0, 1, 0];
        }
        let mut second = simple_font("deep", 0, 1, 50);
        second.sample_data_24 = Some(vec![0; second.sample_points()]);

        // when
        let merged = merge(&[first, second], None).unwrap();

        // then
        assert_eq!(merged.version(), Some((2, 4)));
        assert!(merged.sm24_usable());
    }

    #[test]
    fn merge_should_keep_duplicate_addresses_when_inputs_collide() {
        // given: both fonts use bank 0, program 0
        let fonts = [simple_font("one", 0, 0, 80), simple_font("two", 0, 0, 80)];

        // when
        let merged = merge(&fonts, None).unwrap();
        let issues = validate(&merged);

        // then: both survive, the validator warns about the duplicate
        assert_eq!(merged.presets.len(), 2);
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("duplicate bank:program"))
        );
    }

    #[test]
    fn merge_should_rename_bank_when_name_given() {
        // given
        let fonts = [simple_font("one", 0, 0, 80), simple_font("two", 0, 1, 80)];

        // when
        let merged = merge(&fonts, Some("Combined")).unwrap();

        // then: renamed, and provenance stamped like extraction
        assert_eq!(merged.name().as_deref(), Some("Combined"));
        let isft = String::from_utf8_lossy(merged.info_chunk(*b"ISFT").unwrap()).to_string();
        assert!(isft.contains("sf2-cutter v"));
    }

    #[test]
    fn merge_should_shift_compressed_offsets_by_bytes_when_merging_sf3_input() {
        // given: a plain font followed by an SF3-style font whose compressed
        // sample's start/end are byte offsets and loops decoded-relative
        let first = simple_font("plain", 0, 0, 100);
        let mut second = simple_font("deep", 0, 1, 30);
        if let Some(chunk) = second.info.iter_mut().find(|c| c.id == *b"ifil") {
            chunk.data = vec![3, 0, 0, 0];
        }
        second.samples[0].sample_type = crate::model::SAMPLE_TYPE_COMPRESSED | 1;
        second.samples[0].start = 0;
        second.samples[0].end = 50;
        second.samples[0].start_loop = 10;
        second.samples[0].end_loop = 90;

        // when
        let merged = merge(&[first.clone(), second], None).unwrap();

        // then: byte shift for the stream, loops untouched
        let first_bytes = u32::try_from(first.sample_data.len()).unwrap();
        let compressed = &merged.samples[1];
        assert_eq!(compressed.start, first_bytes);
        assert_eq!(compressed.end, first_bytes + 50);
        assert_eq!(compressed.start_loop, 10);
        assert_eq!(compressed.end_loop, 90);
        // and the plain first font kept point-based shifting (offset zero)
        assert_eq!(merged.samples[0].start, first.samples[0].start);
    }

    #[test]
    fn merge_should_pad_sample_data_when_input_length_is_odd() {
        // given: a first font whose smpl chunk has an odd byte length
        let mut first = SoundFont {
            sample_data: vec![1, 2, 3],
            ..SoundFont::default()
        };
        first.info = simple_font("x", 0, 0, 10).info;
        let second = simple_font("snd", 0, 1, 20);
        let second_start = second.samples[0].start;

        // when
        let merged = merge(&[first, second], None).unwrap();

        // then: padded to even, so the second font's points stay exact
        assert_eq!(merged.samples[0].start, 2 + second_start);
        assert_eq!(merged.sample_data.len() % 2, 0);
    }

    #[test]
    fn merge_should_not_pad_sample_data_when_length_is_already_even() {
        // given: a first font with exactly 2 bytes (1 point) of sample data
        let mut first = SoundFont {
            sample_data: vec![1, 2],
            ..SoundFont::default()
        };
        first.info = simple_font("x", 0, 0, 10).info;
        let second = simple_font("snd", 0, 1, 20);
        let second_len = second.sample_data.len();

        // when
        let merged = merge(&[first, second], None).unwrap();

        // then: no spurious pad byte
        assert_eq!(merged.sample_data.len(), 2 + second_len);
        assert_eq!(
            merged.samples[0].start,
            1 + simple_font("snd", 0, 1, 20).samples[0].start
        );
    }

    #[test]
    fn merge_should_bump_version_when_first_font_has_truncated_ifil() {
        // given: a malformed 2-byte ifil in the first font, sm24 in the second
        let mut first = simple_font("broken", 0, 0, 50);
        if let Some(chunk) = first.info.iter_mut().find(|c| c.id == *b"ifil") {
            chunk.data = vec![2, 0];
        }
        let mut second = simple_font("deep", 0, 1, 50);
        second.sample_data_24 = Some(vec![0; second.sample_points()]);

        // when: must not panic, and must repair the version
        let merged = merge(&[first, second], None).unwrap();

        // then
        assert_eq!(merged.version(), Some((2, 4)));
    }

    #[test]
    fn merge_should_keep_ifil_bytes_when_version_is_already_2_04_with_extra_data() {
        // given: an over-long ifil already at 2.04 (extra bytes preserved)
        let mut first = simple_font("long", 0, 0, 50);
        if let Some(chunk) = first.info.iter_mut().find(|c| c.id == *b"ifil") {
            chunk.data = vec![2, 0, 4, 0, 9, 9];
        }
        let mut second = simple_font("deep", 0, 1, 50);
        second.sample_data_24 = Some(vec![0; second.sample_points()]);

        // when
        let merged = merge(&[first, second], None).unwrap();

        // then: not rewritten (2.04 is not below 2.04)
        assert_eq!(merged.info_chunk(*b"ifil").unwrap(), &[2, 0, 4, 0, 9, 9]);
    }

    #[test]
    fn merge_should_sit_exactly_at_index_limit_when_instruments_total_65536() {
        // given: 65536 instruments split across two fonts
        let mut big = SoundFontBuilder::new("big");
        for i in 0..65_535u32 {
            big.add_instrument(&format!("i{i}"), vec![]).unwrap();
        }
        let big = big.build();
        let mut one = SoundFontBuilder::new("one");
        one.add_instrument("last", vec![]).unwrap();
        let one = one.build();

        // when / then: 65536 total is addressable, one more is not
        let merged = merge(&[big.clone(), one.clone()], None).unwrap();
        assert_eq!(merged.instruments.len(), 65_536);
        let result = merge(&[big, one.clone(), one], None);
        assert!(matches!(result, Err(Error::TooManyRecords("instruments"))));
    }

    #[test]
    fn merge_should_fail_when_no_fonts_given() {
        // given / when
        let result = merge(&[], None);

        // then
        assert!(matches!(result, Err(Error::NothingToMerge)));
    }

    #[test]
    fn merge_should_fail_when_combined_instruments_exceed_u16() {
        // given: two fonts whose instrument counts cannot both fit
        let mut builder = SoundFontBuilder::new("big");
        for i in 0..40_000u32 {
            builder.add_instrument(&format!("i{i}"), vec![]).unwrap();
        }
        let big = builder.build();

        // when
        let result = merge(&[big.clone(), big], None);

        // then
        assert!(matches!(result, Err(Error::TooManyRecords(_))));
    }
}
