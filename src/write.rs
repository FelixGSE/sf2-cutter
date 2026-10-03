//! Serialise a [`SoundFont`] model back into `.sf2` bytes.
//!
//! The writer emits canonical form: contiguous, monotonically increasing bag /
//! generator / modulator indices and freshly generated `EOP`/`EOI`/`EOS`
//! terminal records. `write(parse(x))` is byte-identical for files that are
//! already canonical (which well-formed editors produce).

use crate::error::Error;
use crate::model::{FixedName, Generator, Modulator, SoundFont, Zone, record};
use crate::riff;

/// Serialises the font.
///
/// # Errors
///
/// Returns [`Error::TooManyRecords`] when zone/generator/modulator counts
/// exceed what 16-bit SF2 indices can address, or [`Error::TooLarge`] when a
/// chunk exceeds a 32-bit RIFF size field.
pub fn write(font: &SoundFont) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(usize::try_from(file_size(font)).unwrap_or(0));
    let riff_at = riff::begin_container(&mut out, riff::RIFF, *b"sfbk");

    let info_at = riff::begin_container(&mut out, riff::LIST, *b"INFO");
    for chunk in &font.info {
        riff::push_chunk(&mut out, chunk.id, &chunk.data)?;
    }
    riff::end_container(&mut out, info_at)?;

    let sdta_at = riff::begin_container(&mut out, riff::LIST, *b"sdta");
    riff::push_chunk(&mut out, *b"smpl", &font.sample_data)?;
    if let Some(data_24) = &font.sample_data_24 {
        // SF2.04 §6.3: the declared sm24 size is the point count rounded up
        // to even, i.e. a pad byte must live *inside* the chunk size or
        // conformant readers discard the whole 24-bit extension.
        if data_24.len() % 2 == 1 {
            let mut padded = Vec::with_capacity(data_24.len() + 1);
            padded.extend_from_slice(data_24);
            padded.push(0);
            riff::push_chunk(&mut out, *b"sm24", &padded)?;
        } else {
            riff::push_chunk(&mut out, *b"sm24", data_24)?;
        }
    }
    riff::end_container(&mut out, sdta_at)?;

    let pdta_at = riff::begin_container(&mut out, riff::LIST, *b"pdta");
    for (id, data) in flatten_pdta(font)? {
        riff::push_chunk(&mut out, id, &data)?;
    }
    riff::end_container(&mut out, pdta_at)?;

    riff::end_container(&mut out, riff_at)?;
    Ok(out)
}

/// Size in bytes that [`write`] will produce for this font.
#[must_use]
pub fn file_size(font: &SoundFont) -> u64 {
    let info_body: u64 = font.info.iter().map(|c| chunk_size(c.data.len())).sum();
    let sdta_body = chunk_size(font.sample_data.len())
        + font
            .sample_data_24
            .as_ref()
            .map_or(0, |d| chunk_size(d.len()));
    let (preset_zones, preset_gens, preset_mods) =
        zone_totals(font.presets.iter().map(|p| &p.zones));
    let (inst_zones, inst_gens, inst_mods) = zone_totals(font.instruments.iter().map(|i| &i.zones));
    let pdta_body = chunk_size((font.presets.len() + 1) * record::PHDR)
        + chunk_size((preset_zones + 1) * record::BAG)
        + chunk_size((preset_mods + 1) * record::MOD)
        + chunk_size((preset_gens + 1) * record::GEN)
        + chunk_size((font.instruments.len() + 1) * record::INST)
        + chunk_size((inst_zones + 1) * record::BAG)
        + chunk_size((inst_mods + 1) * record::MOD)
        + chunk_size((inst_gens + 1) * record::GEN)
        + chunk_size((font.samples.len() + 1) * record::SHDR);
    // RIFF header + form, then three LIST containers (header + form each).
    12 + 3 * 12 + info_body + sdta_body + pdta_body
}

fn chunk_size(payload: usize) -> u64 {
    8 + payload as u64 + (payload & 1) as u64
}

fn zone_totals<'a>(zone_lists: impl Iterator<Item = &'a Vec<Zone>>) -> (usize, usize, usize) {
    let mut zones = 0;
    let mut gens = 0;
    let mut mods = 0;
    for list in zone_lists {
        zones += list.len();
        gens += list.iter().map(|z| z.gens.len()).sum::<usize>();
        mods += list.iter().map(|z| z.mods.len()).sum::<usize>();
    }
    (zones, gens, mods)
}

/// Flattens structured zones into the bag/gen/mod arrays plus terminals.
struct FlatZones {
    bags: Vec<u8>,
    gens: Vec<u8>,
    mods: Vec<u8>,
    bag_count: u16,
    gen_count: u16,
    mod_count: u16,
}

impl FlatZones {
    fn new() -> Self {
        Self {
            bags: Vec::new(),
            gens: Vec::new(),
            mods: Vec::new(),
            bag_count: 0,
            gen_count: 0,
            mod_count: 0,
        }
    }

    fn append(&mut self, zones: &[Zone], what: &'static str) -> Result<(), Error> {
        for zone in zones {
            self.bags.extend_from_slice(&self.gen_count.to_le_bytes());
            self.bags.extend_from_slice(&self.mod_count.to_le_bytes());
            for generator in &zone.gens {
                push_gen(&mut self.gens, *generator);
            }
            for modulator in &zone.mods {
                push_mod(&mut self.mods, modulator);
            }
            self.bag_count = bump(self.bag_count, 1, what)?;
            self.gen_count = bump(self.gen_count, zone.gens.len(), what)?;
            self.mod_count = bump(self.mod_count, zone.mods.len(), what)?;
        }
        Ok(())
    }

    fn finish(mut self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        self.bags.extend_from_slice(&self.gen_count.to_le_bytes());
        self.bags.extend_from_slice(&self.mod_count.to_le_bytes());
        self.gens.extend_from_slice(&[0; record::GEN]);
        self.mods.extend_from_slice(&[0; record::MOD]);
        (self.bags, self.gens, self.mods)
    }
}

fn bump(counter: u16, by: usize, what: &'static str) -> Result<u16, Error> {
    u16::try_from(by)
        .ok()
        .and_then(|by| counter.checked_add(by))
        .ok_or(Error::TooManyRecords(what))
}

fn push_gen(out: &mut Vec<u8>, generator: Generator) {
    out.extend_from_slice(&generator.oper.to_le_bytes());
    out.extend_from_slice(&generator.amount.to_le_bytes());
}

fn push_mod(out: &mut Vec<u8>, modulator: &Modulator) {
    out.extend_from_slice(&modulator.src_oper.to_le_bytes());
    out.extend_from_slice(&modulator.dest_oper.to_le_bytes());
    out.extend_from_slice(&modulator.amount.to_le_bytes());
    out.extend_from_slice(&modulator.amount_src_oper.to_le_bytes());
    out.extend_from_slice(&modulator.trans_oper.to_le_bytes());
}

fn push_name(out: &mut Vec<u8>, name: FixedName) {
    out.extend_from_slice(&name.0);
}

type PdtaChunks = Vec<([u8; 4], Vec<u8>)>;

fn flatten_pdta(font: &SoundFont) -> Result<PdtaChunks, Error> {
    let (phdr, pbag, pgen, pmod) = flatten_presets(font)?;
    let (inst, ibag, igen, imod) = flatten_instruments(font)?;
    let shdr = flatten_samples(font);
    Ok(vec![
        (*b"phdr", phdr),
        (*b"pbag", pbag),
        (*b"pmod", pmod),
        (*b"pgen", pgen),
        (*b"inst", inst),
        (*b"ibag", ibag),
        (*b"imod", imod),
        (*b"igen", igen),
        (*b"shdr", shdr),
    ])
}

type FlatRecords = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

fn flatten_presets(font: &SoundFont) -> Result<FlatRecords, Error> {
    let mut phdr = Vec::new();
    let mut zones = FlatZones::new();
    for preset in &font.presets {
        push_name(&mut phdr, preset.name);
        phdr.extend_from_slice(&preset.program.to_le_bytes());
        phdr.extend_from_slice(&preset.bank.to_le_bytes());
        phdr.extend_from_slice(&zones.bag_count.to_le_bytes());
        phdr.extend_from_slice(&preset.library.to_le_bytes());
        phdr.extend_from_slice(&preset.genre.to_le_bytes());
        phdr.extend_from_slice(&preset.morphology.to_le_bytes());
        zones.append(&preset.zones, "preset zones")?;
    }
    push_name(&mut phdr, FixedName::from_text("EOP"));
    phdr.extend_from_slice(&0u16.to_le_bytes());
    phdr.extend_from_slice(&0u16.to_le_bytes());
    phdr.extend_from_slice(&zones.bag_count.to_le_bytes());
    phdr.extend_from_slice(&[0; 12]);
    let (pbag, pgen, pmod) = zones.finish();
    Ok((phdr, pbag, pgen, pmod))
}

fn flatten_instruments(font: &SoundFont) -> Result<FlatRecords, Error> {
    let mut inst = Vec::new();
    let mut zones = FlatZones::new();
    for instrument in &font.instruments {
        push_name(&mut inst, instrument.name);
        inst.extend_from_slice(&zones.bag_count.to_le_bytes());
        zones.append(&instrument.zones, "instrument zones")?;
    }
    push_name(&mut inst, FixedName::from_text("EOI"));
    inst.extend_from_slice(&zones.bag_count.to_le_bytes());
    let (ibag, igen, imod) = zones.finish();
    Ok((inst, ibag, igen, imod))
}

fn flatten_samples(font: &SoundFont) -> Vec<u8> {
    let mut shdr = Vec::new();
    for sample in &font.samples {
        push_name(&mut shdr, sample.name);
        shdr.extend_from_slice(&sample.start.to_le_bytes());
        shdr.extend_from_slice(&sample.end.to_le_bytes());
        shdr.extend_from_slice(&sample.start_loop.to_le_bytes());
        shdr.extend_from_slice(&sample.end_loop.to_le_bytes());
        shdr.extend_from_slice(&sample.sample_rate.to_le_bytes());
        shdr.push(sample.original_pitch);
        shdr.extend_from_slice(&sample.pitch_correction.to_le_bytes());
        shdr.extend_from_slice(&sample.sample_link.to_le_bytes());
        shdr.extend_from_slice(&sample.sample_type.to_le_bytes());
    }
    push_name(&mut shdr, FixedName::from_text("EOS"));
    shdr.extend_from_slice(&[0; record::SHDR - crate::model::NAME_LEN]);
    shdr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::test_font;
    use crate::model::{Preset, Zone};
    use crate::parse::parse;

    #[test]
    fn write_then_parse_should_return_equal_model_when_font_is_synthetic() {
        // given
        let font = test_font();

        // when
        let bytes = write(&font).unwrap();
        let reparsed = parse(&bytes).unwrap();

        // then
        assert_eq!(reparsed, font);
    }

    #[test]
    fn write_should_be_idempotent_when_reserialising_its_own_output() {
        // given
        let bytes = write(&test_font()).unwrap();

        // when
        let again = write(&parse(&bytes).unwrap()).unwrap();

        // then
        assert_eq!(again, bytes);
    }

    #[test]
    fn file_size_should_match_written_length_when_font_is_synthetic() {
        // given
        let font = test_font();

        // when
        let bytes = write(&font).unwrap();

        // then
        assert_eq!(file_size(&font), bytes.len() as u64);
    }

    #[test]
    fn write_should_declare_even_sm24_size_when_length_is_odd() {
        // given
        let mut font = test_font();
        font.sample_data_24 = Some(vec![7; 33]);

        // when
        let reparsed = parse(&write(&font).unwrap()).unwrap();

        // then: the pad byte is inside the declared size
        let sliced = reparsed.sample_data_24.unwrap();
        assert_eq!(sliced.len(), 34);
        assert_eq!(sliced[33], 0);
        assert_eq!(&sliced[..33], &[7; 33][..]);
    }

    #[test]
    fn file_size_should_match_written_length_when_chunks_have_odd_sizes() {
        // given: an odd INFO chunk and an odd sm24 chunk
        let mut font = test_font();
        font.info.push(crate::model::InfoChunk {
            id: *b"ICMT",
            data: b"odd".to_vec(),
        });
        font.sample_data_24 = Some(vec![7; 33]);

        // when
        let bytes = write(&font).unwrap();

        // then
        assert_eq!(file_size(&font), bytes.len() as u64);
    }

    #[test]
    fn write_should_emit_eop_terminal_when_font_has_presets() {
        // given
        let font = test_font();

        // when
        let bytes = write(&font).unwrap();

        // then: locate phdr inside pdta and inspect its final record
        let (_, body) = crate::riff::root(&bytes).unwrap();
        let lists = crate::riff::chunks(body).unwrap();
        let pdta = lists
            .iter()
            .find_map(|c| {
                crate::riff::list_body(c)
                    .ok()
                    .filter(|(form, _)| form == b"pdta")
            })
            .unwrap()
            .1;
        let phdr = crate::riff::chunks(pdta)
            .unwrap()
            .into_iter()
            .find(|c| c.id == *b"phdr")
            .unwrap()
            .data
            .to_vec();
        let terminal = &phdr[phdr.len() - 38..];
        assert_eq!(&terminal[..4], b"EOP\0");
    }

    #[test]
    fn write_should_fail_when_zone_count_exceeds_sixteen_bit_index() {
        // given: a single preset with 65536 empty zones
        let mut font = test_font();
        font.presets.push(Preset {
            name: crate::model::FixedName::from_text("huge"),
            program: 1,
            bank: 1,
            library: 0,
            genre: 0,
            morphology: 0,
            zones: vec![Zone::default(); 65_536],
        });

        // when
        let result = write(&font);

        // then
        assert!(matches!(result, Err(Error::TooManyRecords("preset zones"))));
    }
}
