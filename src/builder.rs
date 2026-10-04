//! Programmatic construction of [`SoundFont`] models.
//!
//! Primarily used to build small synthetic soundfonts in tests, but part of
//! the public API because it is equally useful for assembling fonts from raw
//! sample data.

use crate::error::Error;
use crate::model::Instrument;
use crate::model::{
    FixedName, GEN_INSTRUMENT, GEN_SAMPLE_ID, Generator, InfoChunk, Modulator, Preset,
    SampleHeader, SoundFont, Zone,
};

/// Zero sample points appended after every sample, as the spec requires
/// ("a minimum of 46 data points ... should follow each sample").
pub const GUARD_POINTS: usize = 46;

/// Incrementally assembles a [`SoundFont`].
#[derive(Debug, Clone)]
pub struct SoundFontBuilder {
    font: SoundFont,
}

impl SoundFontBuilder {
    /// Starts a font with the mandatory `INFO` chunks (`ifil` 2.04, `isng`,
    /// and `INAM` set to `name`).
    #[must_use]
    pub fn new(name: &str) -> Self {
        let mut inam = name.as_bytes().to_vec();
        inam.push(0);
        if inam.len() % 2 == 1 {
            inam.push(0);
        }
        Self {
            font: SoundFont {
                info: vec![
                    InfoChunk {
                        id: *b"ifil",
                        data: vec![2, 0, 4, 0],
                    },
                    InfoChunk {
                        id: *b"isng",
                        data: b"EMu10K1\0".to_vec(),
                    },
                    InfoChunk {
                        id: *b"INAM",
                        data: inam,
                    },
                ],
                ..SoundFont::default()
            },
        }
    }

    /// Appends a mono 16-bit sample (plus guard points) and returns its index.
    ///
    /// The whole sample is used as the loop range and the pitch correction is
    /// zero; adjust the returned header via `font.samples[index]` after
    /// [`Self::build`] when you need real loop points.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TooManyRecords`] when the sample index would not fit
    /// in a 16-bit generator amount, or [`Error::TooLarge`] when the
    /// accumulated sample data would exceed 32-bit sample offsets.
    pub fn add_sample(
        &mut self,
        name: &str,
        pcm: &[i16],
        sample_rate: u32,
        original_pitch: u8,
    ) -> Result<u16, Error> {
        let index =
            u16::try_from(self.font.samples.len()).map_err(|_| Error::TooManyRecords("samples"))?;
        let start =
            u32::try_from(self.font.sample_points()).map_err(|_| Error::TooLarge("smpl"))?;
        let len = u32::try_from(pcm.len()).map_err(|_| Error::TooLarge("sample"))?;
        for value in pcm {
            self.font
                .sample_data
                .extend_from_slice(&value.to_le_bytes());
        }
        self.font
            .sample_data
            .extend_from_slice(&[0; GUARD_POINTS * 2]);
        self.font.samples.push(SampleHeader {
            name: FixedName::from_text(name),
            start,
            end: start + len,
            start_loop: start,
            end_loop: start + len,
            sample_rate,
            original_pitch,
            pitch_correction: 0,
            sample_link: 0,
            sample_type: 1,
        });
        Ok(index)
    }

    /// Marks two already-added samples as a stereo pair (left, right).
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexOutOfBounds`] when either index does not name an
    /// existing sample or both name the same sample.
    pub fn link_stereo(&mut self, left: u16, right: u16) -> Result<(), Error> {
        let count = self.font.samples.len();
        if usize::from(left) >= count || usize::from(right) >= count || left == right {
            return Err(Error::IndexOutOfBounds {
                what: "stereo link",
                index: usize::from(left.max(right)),
                max: count.saturating_sub(1),
            });
        }
        if let Some(header) = self.font.samples.get_mut(usize::from(left)) {
            header.sample_type = 4;
            header.sample_link = right;
        }
        if let Some(header) = self.font.samples.get_mut(usize::from(right)) {
            header.sample_type = 2;
            header.sample_link = left;
        }
        Ok(())
    }

    /// Adds an instrument with the given zones and returns its index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TooManyRecords`] when the instrument index would not
    /// fit in a 16-bit generator amount.
    pub fn add_instrument(&mut self, name: &str, zones: Vec<Zone>) -> Result<u16, Error> {
        let index = u16::try_from(self.font.instruments.len())
            .map_err(|_| Error::TooManyRecords("instruments"))?;
        self.font.instruments.push(Instrument {
            name: FixedName::from_text(name),
            zones,
        });
        Ok(index)
    }

    /// Adds a preset with the given zones.
    pub fn add_preset(&mut self, name: &str, bank: u16, program: u16, zones: Vec<Zone>) {
        self.font.presets.push(Preset {
            name: FixedName::from_text(name),
            program,
            bank,
            library: 0,
            genre: 0,
            morphology: 0,
            zones,
        });
    }

    /// Finishes construction.
    #[must_use]
    pub fn build(self) -> SoundFont {
        self.font
    }
}

/// An instrument zone that plays the given sample.
#[must_use]
pub fn sample_zone(sample: u16) -> Zone {
    Zone {
        gens: vec![Generator {
            oper: GEN_SAMPLE_ID,
            amount: sample,
        }],
        mods: vec![],
    }
}

/// A preset zone that plays the given instrument.
#[must_use]
pub fn instrument_zone(instrument: u16) -> Zone {
    Zone {
        gens: vec![Generator {
            oper: GEN_INSTRUMENT,
            amount: instrument,
        }],
        mods: vec![],
    }
}

/// A global zone carrying the given generators and modulators (no
/// instrument/sample reference).
#[must_use]
pub const fn global_zone(gens: Vec<Generator>, mods: Vec<Modulator>) -> Zone {
    Zone { gens, mods }
}

/// Builds the small font used across the test suite.
///
/// It contains a mono "Piano" (bank 0, program 0), a stereo "Strings"
/// (bank 0, program 48) with a global zone and a modulator, and a "Drums"
/// kit on bank 128.
///
/// # Panics
///
/// Panics when the fixture cannot be built; only usable in tests.
#[cfg(test)]
#[must_use]
pub fn test_font() -> SoundFont {
    let mut builder = SoundFontBuilder::new("Fixture Font");
    let piano_pcm: Vec<i16> = (0..200).map(|i| i * 50).collect();
    let piano_sample = builder
        .add_sample("piano-c4", &piano_pcm, 44100, 60)
        .unwrap();
    let left_pcm: Vec<i16> = (0..300).map(|i| i * 30).collect();
    let right_pcm: Vec<i16> = (0..300).map(|i| i * -30).collect();
    let strings_l = builder
        .add_sample("strings-l", &left_pcm, 44100, 57)
        .unwrap();
    let strings_r = builder
        .add_sample("strings-r", &right_pcm, 44100, 57)
        .unwrap();
    builder.link_stereo(strings_l, strings_r).unwrap();
    let drum_pcm: Vec<i16> = (0..100).map(|i| (i % 7) * 1000).collect();
    let drum_sample = builder.add_sample("kick", &drum_pcm, 22050, 36).unwrap();

    let piano_inst = builder
        .add_instrument("Piano Inst", vec![sample_zone(piano_sample)])
        .unwrap();
    let strings_inst = builder
        .add_instrument(
            "Strings Inst",
            vec![
                global_zone(
                    vec![Generator {
                        oper: 8,
                        amount: 5000,
                    }],
                    vec![Modulator {
                        src_oper: 0x0502,
                        dest_oper: 48,
                        amount: 960,
                        amount_src_oper: 0,
                        trans_oper: 0,
                    }],
                ),
                sample_zone(strings_l),
                sample_zone(strings_r),
            ],
        )
        .unwrap();
    let drums_inst = builder
        .add_instrument("Drums Inst", vec![sample_zone(drum_sample)])
        .unwrap();

    builder.add_preset("Bright Piano", 0, 0, vec![instrument_zone(piano_inst)]);
    builder.add_preset("Slow Strings", 0, 48, vec![instrument_zone(strings_inst)]);
    builder.add_preset("Standard Kit", 128, 0, vec![instrument_zone(drums_inst)]);
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_should_append_guard_points_when_sample_added() {
        // given
        let mut builder = SoundFontBuilder::new("t");

        // when
        let index = builder.add_sample("s", &[1, 2, 3], 44100, 60).unwrap();

        // then
        let font = builder.build();
        assert_eq!(index, 0);
        assert_eq!(font.sample_data.len(), (3 + GUARD_POINTS) * 2);
        assert_eq!(font.samples[0].start, 0);
        assert_eq!(font.samples[0].end, 3);
    }

    #[test]
    fn builder_should_offset_sample_start_when_second_sample_added() {
        // given
        let mut builder = SoundFontBuilder::new("t");
        builder.add_sample("a", &[1, 2], 44100, 60).unwrap();

        // when
        builder.add_sample("b", &[3], 44100, 60).unwrap();

        // then
        let font = builder.build();
        let expected_start = u32::try_from(2 + GUARD_POINTS).unwrap();
        assert_eq!(font.samples[1].start, expected_start);
        assert_eq!(font.samples[1].end, expected_start + 1);
    }

    #[test]
    fn builder_should_cross_link_samples_when_stereo_pair_declared() {
        // given
        let mut builder = SoundFontBuilder::new("t");
        let left = builder.add_sample("l", &[1], 44100, 60).unwrap();
        let right = builder.add_sample("r", &[1], 44100, 60).unwrap();

        // when
        builder.link_stereo(left, right).unwrap();

        // then
        let font = builder.build();
        assert_eq!(font.samples[0].sample_type, 4);
        assert_eq!(font.samples[0].sample_link, right);
        assert_eq!(font.samples[1].sample_type, 2);
        assert_eq!(font.samples[1].sample_link, left);
    }

    #[test]
    fn global_zone_should_carry_given_generators_and_modulators_when_built() {
        // given
        let gens = vec![Generator { oper: 8, amount: 1 }];
        let mods = vec![Modulator {
            src_oper: 2,
            dest_oper: 48,
            amount: 10,
            amount_src_oper: 0,
            trans_oper: 0,
        }];

        // when
        let zone = global_zone(gens.clone(), mods.clone());

        // then
        assert_eq!(zone.gens, gens);
        assert_eq!(zone.mods, mods);
    }

    #[test]
    fn builder_should_fail_when_stereo_link_is_invalid() {
        // given
        let mut builder = SoundFontBuilder::new("t");
        let sample = builder.add_sample("s", &[1], 44100, 60).unwrap();

        // when / then: out-of-range partner and self-link are both rejected
        assert!(matches!(
            builder.link_stereo(sample, 99),
            Err(Error::IndexOutOfBounds { .. })
        ));
        assert!(matches!(
            builder.link_stereo(sample, sample),
            Err(Error::IndexOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_font_should_contain_three_presets_when_built() {
        // given / when
        let font = test_font();

        // then
        assert_eq!(font.presets.len(), 3);
        assert_eq!(font.instruments.len(), 3);
        assert_eq!(font.samples.len(), 4);
        assert_eq!(font.version(), Some((2, 4)));
    }
}
