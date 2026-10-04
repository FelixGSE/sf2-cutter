//! Parse `.sf2` bytes into a [`SoundFont`] model.
//!
//! Parsing is structural: it rejects malformed RIFF layout, bad record sizes,
//! missing terminal records, and non-monotonic or out-of-range zone indices.
//! Cross-reference checks (e.g. a preset zone pointing at a missing
//! instrument) are the job of [`crate::validate`].

use crate::error::Error;
use crate::model::{
    FixedName, Generator, Instrument, Modulator, Preset, SampleHeader, SoundFont, Zone, record,
};
use crate::riff::{self, Chunk};

/// Parses a complete `.sf2` file.
///
/// # Errors
///
/// Returns an [`Error`] when the byte stream is not a structurally valid
/// `SoundFont` 2 file.
pub fn parse(bytes: &[u8]) -> Result<SoundFont, Error> {
    let (form, body) = riff::root(bytes)?;
    if form != *b"sfbk" {
        return Err(Error::NotSoundFont(format!(
            "form type is `{}`, expected `sfbk`",
            riff::fourcc(form)
        )));
    }
    let (info_body, sdta_body, pdta_body) = top_level_lists(body)?;
    let info = parse_info(info_body)?;
    let (sample_data, sample_data_24) = parse_sdta(sdta_body)?;
    let pdta = Pdta::parse(pdta_body)?;
    Ok(SoundFont {
        info,
        sample_data,
        sample_data_24,
        presets: build_presets(&pdta)?,
        instruments: build_instruments(&pdta)?,
        samples: pdta.samples,
    })
}

type InfoChunks = Vec<crate::model::InfoChunk>;

type TopLevelLists<'a> = (&'a [u8], &'a [u8], &'a [u8]);

fn top_level_lists(body: &[u8]) -> Result<TopLevelLists<'_>, Error> {
    let mut info = None;
    let mut sdta = None;
    let mut pdta = None;
    for chunk in riff::chunks(body)? {
        if chunk.id != riff::LIST {
            continue;
        }
        let (list_form, list_data) = riff::list_body(&chunk)?;
        let slot = match &list_form {
            b"INFO" => &mut info,
            b"sdta" => &mut sdta,
            b"pdta" => &mut pdta,
            _ => continue,
        };
        if slot.is_none() {
            *slot = Some(list_data);
        }
    }
    Ok((
        info.ok_or(Error::MissingChunk("INFO"))?,
        sdta.ok_or(Error::MissingChunk("sdta"))?,
        pdta.ok_or(Error::MissingChunk("pdta"))?,
    ))
}

fn parse_info(body: &[u8]) -> Result<InfoChunks, Error> {
    let chunks: InfoChunks = riff::chunks(body)?
        .into_iter()
        .map(|c| crate::model::InfoChunk {
            id: c.id,
            data: c.data.to_vec(),
        })
        .collect();
    if !chunks.iter().any(|c| c.id == *b"ifil") {
        return Err(Error::MissingChunk("ifil"));
    }
    Ok(chunks)
}

fn parse_sdta(body: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), Error> {
    let mut smpl = Vec::new();
    let mut sm24 = None;
    for chunk in riff::chunks(body)? {
        match &chunk.id {
            b"smpl" => smpl = chunk.data.to_vec(),
            b"sm24" => sm24 = Some(chunk.data.to_vec()),
            _ => {}
        }
    }
    Ok((smpl, sm24))
}

/// Raw `pdta` record arrays, terminal records still included except for
/// `samples`, where the terminal is already dropped.
struct Pdta {
    phdr: Vec<RawPresetHeader>,
    pbag: Vec<RawBag>,
    pmod: Vec<Modulator>,
    pgen: Vec<Generator>,
    inst: Vec<RawInstrumentHeader>,
    ibag: Vec<RawBag>,
    imod: Vec<Modulator>,
    igen: Vec<Generator>,
    samples: Vec<SampleHeader>,
}

struct RawPresetHeader {
    name: FixedName,
    program: u16,
    bank: u16,
    bag_ndx: u16,
    library: u32,
    genre: u32,
    morphology: u32,
}

struct RawInstrumentHeader {
    name: FixedName,
    bag_ndx: u16,
}

struct RawBag {
    gen_ndx: u16,
    mod_ndx: u16,
}

impl Pdta {
    fn parse(body: &[u8]) -> Result<Self, Error> {
        let chunks = riff::chunks(body)?;
        let phdr = decode_phdr(find_chunk(&chunks, *b"phdr", "phdr")?)?;
        let pbag = decode_bags(find_chunk(&chunks, *b"pbag", "pbag")?, "pbag")?;
        let pmod = decode_mods(find_chunk(&chunks, *b"pmod", "pmod")?, "pmod")?;
        let pgen = decode_gens(find_chunk(&chunks, *b"pgen", "pgen")?, "pgen")?;
        let inst = decode_inst(find_chunk(&chunks, *b"inst", "inst")?)?;
        let ibag = decode_bags(find_chunk(&chunks, *b"ibag", "ibag")?, "ibag")?;
        let imod = decode_mods(find_chunk(&chunks, *b"imod", "imod")?, "imod")?;
        let igen = decode_gens(find_chunk(&chunks, *b"igen", "igen")?, "igen")?;
        let mut samples = decode_shdr(find_chunk(&chunks, *b"shdr", "shdr")?)?;

        if let Some(last) = phdr.last() {
            ensure_header_terminal(last.name, last.bag_ndx, pbag.len(), *b"EOP", "phdr")?;
        }
        if let Some(last) = inst.last() {
            ensure_header_terminal(last.name, last.bag_ndx, ibag.len(), *b"EOI", "inst")?;
        }
        if let Some(terminal) = samples.pop() {
            let zeroed = terminal.start == 0 && terminal.end == 0;
            if !(terminal.name.0.starts_with(b"EOS") || zeroed) {
                return Err(Error::MissingTerminal("shdr"));
            }
        }

        Ok(Self {
            phdr,
            pbag,
            pmod,
            pgen,
            inst,
            ibag,
            imod,
            igen,
            samples,
        })
    }
}

/// The last record of a header chunk must be its terminal: named `EOP`/`EOI`,
/// or at least pointing at the sentinel bag. Dropping a real record here
/// would silently lose data, so anything else is rejected.
fn ensure_header_terminal(
    name: FixedName,
    bag_ndx: u16,
    bag_count: usize,
    magic: [u8; 3],
    chunk: &'static str,
) -> Result<(), Error> {
    if name.0.starts_with(&magic) || usize::from(bag_ndx) + 1 == bag_count {
        Ok(())
    } else {
        Err(Error::MissingTerminal(chunk))
    }
}

fn find_chunk<'a>(
    chunks: &[Chunk<'a>],
    id: [u8; 4],
    name: &'static str,
) -> Result<&'a [u8], Error> {
    chunks
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.data)
        .ok_or(Error::MissingChunk(name))
}

const fn ensure_records(data: &[u8], record_size: usize, chunk: &'static str) -> Result<(), Error> {
    if !data.len().is_multiple_of(record_size) {
        return Err(Error::BadRecordSize {
            chunk,
            size: data.len(),
            record_size,
        });
    }
    if data.is_empty() {
        return Err(Error::MissingTerminal(chunk));
    }
    Ok(())
}

fn read_name(record: &[u8]) -> FixedName {
    let mut name = [0u8; crate::model::NAME_LEN];
    name.copy_from_slice(&record[..crate::model::NAME_LEN]);
    FixedName(name)
}

const fn le16(record: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([record[offset], record[offset + 1]])
}

const fn le32(record: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        record[offset],
        record[offset + 1],
        record[offset + 2],
        record[offset + 3],
    ])
}

fn decode_phdr(data: &[u8]) -> Result<Vec<RawPresetHeader>, Error> {
    ensure_records(data, record::PHDR, "phdr")?;
    Ok(data
        .as_chunks::<{ record::PHDR }>()
        .0
        .iter()
        .map(|r| RawPresetHeader {
            name: read_name(r),
            program: le16(r, 20),
            bank: le16(r, 22),
            bag_ndx: le16(r, 24),
            library: le32(r, 26),
            genre: le32(r, 30),
            morphology: le32(r, 34),
        })
        .collect())
}

fn decode_inst(data: &[u8]) -> Result<Vec<RawInstrumentHeader>, Error> {
    ensure_records(data, record::INST, "inst")?;
    Ok(data
        .as_chunks::<{ record::INST }>()
        .0
        .iter()
        .map(|r| RawInstrumentHeader {
            name: read_name(r),
            bag_ndx: le16(r, 20),
        })
        .collect())
}

fn decode_bags(data: &[u8], chunk: &'static str) -> Result<Vec<RawBag>, Error> {
    ensure_records(data, record::BAG, chunk)?;
    Ok(data
        .as_chunks::<{ record::BAG }>()
        .0
        .iter()
        .map(|r| RawBag {
            gen_ndx: le16(r, 0),
            mod_ndx: le16(r, 2),
        })
        .collect())
}

fn decode_mods(data: &[u8], chunk: &'static str) -> Result<Vec<Modulator>, Error> {
    ensure_records(data, record::MOD, chunk)?;
    Ok(data
        .as_chunks::<{ record::MOD }>()
        .0
        .iter()
        .map(|r| Modulator {
            src_oper: le16(r, 0),
            dest_oper: le16(r, 2),
            amount: i16::from_le_bytes([r[4], r[5]]),
            amount_src_oper: le16(r, 6),
            trans_oper: le16(r, 8),
        })
        .collect())
}

fn decode_gens(data: &[u8], chunk: &'static str) -> Result<Vec<Generator>, Error> {
    ensure_records(data, record::GEN, chunk)?;
    Ok(data
        .as_chunks::<{ record::GEN }>()
        .0
        .iter()
        .map(|r| Generator {
            oper: le16(r, 0),
            amount: le16(r, 2),
        })
        .collect())
}

fn decode_shdr(data: &[u8]) -> Result<Vec<SampleHeader>, Error> {
    ensure_records(data, record::SHDR, "shdr")?;
    Ok(data
        .as_chunks::<{ record::SHDR }>()
        .0
        .iter()
        .map(|r| SampleHeader {
            name: read_name(r),
            start: le32(r, 20),
            end: le32(r, 24),
            start_loop: le32(r, 28),
            end_loop: le32(r, 32),
            sample_rate: le32(r, 36),
            original_pitch: r[40],
            pitch_correction: i8::from_le_bytes([r[41]]),
            sample_link: le16(r, 42),
            sample_type: le16(r, 44),
        })
        .collect())
}

/// Turns a monotonic index list (including the terminal sentinel entry) into
/// `starts.len() - 1` ranges, each bounded by `max_end`.
fn index_ranges(
    starts: &[u16],
    max_end: usize,
    what: &'static str,
) -> Result<Vec<std::ops::Range<usize>>, Error> {
    let mut ranges = Vec::with_capacity(starts.len().saturating_sub(1));
    for pair in starts.windows(2) {
        let (from, to) = (usize::from(pair[0]), usize::from(pair[1]));
        if from > to {
            return Err(Error::IndexOrder(what));
        }
        if to > max_end {
            return Err(Error::IndexOutOfBounds {
                what,
                index: to,
                max: max_end,
            });
        }
        ranges.push(from..to);
    }
    Ok(ranges)
}

/// Resolves a flat bag/gen/mod triple into one zone per bag (the terminal
/// bag only serves as sentinel).
fn build_zones(
    bags: &[RawBag],
    gens: &[Generator],
    mods: &[Modulator],
    what: &'static str,
) -> Result<Vec<Zone>, Error> {
    let gen_starts: Vec<u16> = bags.iter().map(|b| b.gen_ndx).collect();
    let mod_starts: Vec<u16> = bags.iter().map(|b| b.mod_ndx).collect();
    let gen_ranges = index_ranges(&gen_starts, gens.len() - 1, what)?;
    let mod_ranges = index_ranges(&mod_starts, mods.len() - 1, what)?;
    Ok(gen_ranges
        .into_iter()
        .zip(mod_ranges)
        .map(|(g, m)| Zone {
            gens: gens[g].to_vec(),
            mods: mods[m].to_vec(),
        })
        .collect())
}

fn build_presets(pdta: &Pdta) -> Result<Vec<Preset>, Error> {
    let zones = build_zones(&pdta.pbag, &pdta.pgen, &pdta.pmod, "preset zones")?;
    let bag_starts: Vec<u16> = pdta.phdr.iter().map(|h| h.bag_ndx).collect();
    let ranges = index_ranges(&bag_starts, zones.len(), "preset bag indices")?;
    Ok(pdta
        .phdr
        .iter()
        .zip(ranges)
        .map(|(header, range)| Preset {
            name: header.name,
            program: header.program,
            bank: header.bank,
            library: header.library,
            genre: header.genre,
            morphology: header.morphology,
            zones: zones[range].to_vec(),
        })
        .collect())
}

fn build_instruments(pdta: &Pdta) -> Result<Vec<Instrument>, Error> {
    let zones = build_zones(&pdta.ibag, &pdta.igen, &pdta.imod, "instrument zones")?;
    let bag_starts: Vec<u16> = pdta.inst.iter().map(|h| h.bag_ndx).collect();
    let ranges = index_ranges(&bag_starts, zones.len(), "instrument bag indices")?;
    Ok(pdta
        .inst
        .iter()
        .zip(ranges)
        .map(|(header, range)| Instrument {
            name: header.name,
            zones: zones[range].to_vec(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::riff::{LIST, RIFF, begin_container, end_container, push_chunk};

    /// Byte-level `pdta` building blocks, defaulting to terminal-only chunks.
    struct PdtaBytes {
        phdr: Vec<u8>,
        pbag: Vec<u8>,
        pmod: Vec<u8>,
        pgen: Vec<u8>,
        inst: Vec<u8>,
        ibag: Vec<u8>,
        imod: Vec<u8>,
        igen: Vec<u8>,
        shdr: Vec<u8>,
    }

    impl Default for PdtaBytes {
        fn default() -> Self {
            Self {
                phdr: phdr_record("EOP", 0, 0, 0, 0),
                pbag: bag_record(0, 0),
                pmod: vec![0; 10],
                pgen: vec![0; 4],
                inst: inst_record("EOI", 0),
                ibag: bag_record(0, 0),
                imod: vec![0; 10],
                igen: vec![0; 4],
                shdr: vec![0; 46],
            }
        }
    }

    fn phdr_record(name: &str, program: u16, bank: u16, bag: u16, library: u32) -> Vec<u8> {
        let mut out = FixedName::from_text(name).0.to_vec();
        out.extend_from_slice(&program.to_le_bytes());
        out.extend_from_slice(&bank.to_le_bytes());
        out.extend_from_slice(&bag.to_le_bytes());
        out.extend_from_slice(&library.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }

    fn inst_record(name: &str, bag: u16) -> Vec<u8> {
        let mut out = FixedName::from_text(name).0.to_vec();
        out.extend_from_slice(&bag.to_le_bytes());
        out
    }

    fn bag_record(generator: u16, modulator: u16) -> Vec<u8> {
        let mut out = generator.to_le_bytes().to_vec();
        out.extend_from_slice(&modulator.to_le_bytes());
        out
    }

    fn shdr_record(name: &str, start: u32, end: u32) -> Vec<u8> {
        let mut out = FixedName::from_text(name).0.to_vec();
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&end.to_le_bytes());
        out.extend_from_slice(&[0; 18]);
        out
    }

    fn gen_record(oper: u16, amount: u16) -> Vec<u8> {
        let mut out = oper.to_le_bytes().to_vec();
        out.extend_from_slice(&amount.to_le_bytes());
        out
    }

    fn font_bytes(pdta: &PdtaBytes) -> Vec<u8> {
        font_bytes_with(pdta, true)
    }

    fn font_bytes_with(pdta: &PdtaBytes, with_ifil: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let riff_at = begin_container(&mut out, RIFF, *b"sfbk");
        let info_at = begin_container(&mut out, LIST, *b"INFO");
        if with_ifil {
            push_chunk(&mut out, *b"ifil", &[2, 0, 1, 0]).unwrap();
        }
        push_chunk(&mut out, *b"INAM", b"t\0").unwrap();
        end_container(&mut out, info_at).unwrap();
        let sdta_at = begin_container(&mut out, LIST, *b"sdta");
        push_chunk(&mut out, *b"smpl", &[]).unwrap();
        end_container(&mut out, sdta_at).unwrap();
        let pdta_at = begin_container(&mut out, LIST, *b"pdta");
        for (id, data) in [
            (*b"phdr", &pdta.phdr),
            (*b"pbag", &pdta.pbag),
            (*b"pmod", &pdta.pmod),
            (*b"pgen", &pdta.pgen),
            (*b"inst", &pdta.inst),
            (*b"ibag", &pdta.ibag),
            (*b"imod", &pdta.imod),
            (*b"igen", &pdta.igen),
            (*b"shdr", &pdta.shdr),
        ] {
            push_chunk(&mut out, id, data).unwrap();
        }
        end_container(&mut out, pdta_at).unwrap();
        end_container(&mut out, riff_at).unwrap();
        out
    }

    #[test]
    fn parse_should_return_empty_font_when_only_terminal_records_present() {
        // given
        let bytes = font_bytes(&PdtaBytes::default());

        // when
        let font = parse(&bytes).unwrap();

        // then
        assert_eq!(font.presets, vec![]);
        assert_eq!(font.instruments, vec![]);
        assert_eq!(font.samples, vec![]);
        assert_eq!(font.version(), Some((2, 1)));
    }

    #[test]
    fn parse_should_extract_preset_fields_when_records_are_valid() {
        // given: one preset with one zone holding one instrument generator
        let mut pdta = PdtaBytes::default();
        let mut phdr = phdr_record("Test Preset", 3, 2, 0, 7);
        phdr.extend_from_slice(&phdr_record("EOP", 0, 0, 1, 0));
        pdta.phdr = phdr;
        let mut pbag = bag_record(0, 0);
        pbag.extend_from_slice(&bag_record(1, 0));
        pdta.pbag = pbag;
        let mut pgen = gen_record(crate::model::GEN_INSTRUMENT, 9);
        pgen.extend_from_slice(&gen_record(0, 0));
        pdta.pgen = pgen;

        // when
        let font = parse(&font_bytes(&pdta)).unwrap();

        // then
        assert_eq!(font.presets.len(), 1);
        let preset = &font.presets[0];
        assert_eq!(preset.name.to_display(), "Test Preset");
        assert_eq!(preset.program, 3);
        assert_eq!(preset.bank, 2);
        assert_eq!(preset.library, 7);
        assert_eq!(preset.zones.len(), 1);
        assert_eq!(preset.zones[0].instrument_ref(), Some(9));
    }

    #[test]
    fn parse_should_reject_file_when_form_type_is_not_sfbk() {
        // given
        let mut out = Vec::new();
        let at = begin_container(&mut out, RIFF, *b"WAVE");
        end_container(&mut out, at).unwrap();

        // when
        let result = parse(&out);

        // then
        assert!(matches!(result, Err(Error::NotSoundFont(_))));
    }

    #[test]
    fn parse_should_reject_file_when_pdta_list_is_missing() {
        // given
        let mut out = Vec::new();
        let riff_at = begin_container(&mut out, RIFF, *b"sfbk");
        let info_at = begin_container(&mut out, LIST, *b"INFO");
        push_chunk(&mut out, *b"ifil", &[2, 0, 1, 0]).unwrap();
        end_container(&mut out, info_at).unwrap();
        let sdta_at = begin_container(&mut out, LIST, *b"sdta");
        end_container(&mut out, sdta_at).unwrap();
        end_container(&mut out, riff_at).unwrap();

        // when
        let result = parse(&out);

        // then
        assert!(matches!(result, Err(Error::MissingChunk("pdta"))));
    }

    #[test]
    fn parse_should_reject_file_when_ifil_chunk_is_missing() {
        // given
        let bytes = font_bytes_with(&PdtaBytes::default(), false);

        // when
        let result = parse(&bytes);

        // then
        assert!(matches!(result, Err(Error::MissingChunk("ifil"))));
    }

    #[test]
    fn parse_should_reject_phdr_when_size_is_not_a_record_multiple() {
        // given
        let mut pdta = PdtaBytes::default();
        pdta.phdr.push(0);

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(
            result,
            Err(Error::BadRecordSize { chunk: "phdr", .. })
        ));
    }

    #[test]
    fn parse_should_reject_phdr_when_terminal_record_is_missing() {
        // given
        let pdta = PdtaBytes {
            phdr: Vec::new(),
            ..PdtaBytes::default()
        };

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(result, Err(Error::MissingTerminal("phdr"))));
    }

    #[test]
    fn parse_should_reject_shdr_when_terminal_record_is_a_real_sample() {
        // given: a single shdr record that is clearly audio, not a terminal
        let pdta = PdtaBytes {
            shdr: shdr_record("Real Sample", 10, 500),
            ..PdtaBytes::default()
        };

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(result, Err(Error::MissingTerminal("shdr"))));
    }

    #[test]
    fn parse_should_reject_phdr_when_terminal_points_past_sentinel_bag() {
        // given: a lone phdr record that is neither EOP nor sentinel-aligned
        let pdta = PdtaBytes {
            phdr: phdr_record("Not A Terminal", 0, 0, 5, 0),
            ..PdtaBytes::default()
        };

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(result, Err(Error::MissingTerminal("phdr"))));
    }

    #[test]
    fn parse_should_accept_terminal_when_unnamed_but_sentinel_aligned() {
        // given: a lone phdr record not named EOP whose bag index is the sentinel
        let pdta = PdtaBytes {
            phdr: phdr_record("X", 0, 0, 0, 0),
            ..PdtaBytes::default()
        };

        // when
        let font = parse(&font_bytes(&pdta)).unwrap();

        // then: it is treated as the terminal, leaving zero presets
        assert_eq!(font.presets, vec![]);
    }

    #[test]
    fn parse_should_reject_shdr_terminal_when_only_start_is_zero() {
        // given: an unnamed-terminal candidate with a nonzero end offset
        let pdta = PdtaBytes {
            shdr: shdr_record("Real Sample", 0, 500),
            ..PdtaBytes::default()
        };

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(result, Err(Error::MissingTerminal("shdr"))));
    }

    #[test]
    fn parse_should_reject_zone_when_gen_index_consumes_terminal_record() {
        // given: a sentinel bag pointing exactly at the pgen terminal + 1
        let mut pdta = PdtaBytes::default();
        let mut pbag = bag_record(0, 0);
        pbag.extend_from_slice(&bag_record(1, 0));
        pdta.pbag = pbag;

        // when
        let result = parse(&font_bytes(&pdta));

        // then: the terminal generator record is not addressable zone content
        assert!(matches!(
            result,
            Err(Error::IndexOutOfBounds {
                what: "preset zones",
                ..
            })
        ));
    }

    #[test]
    fn parse_should_reject_zone_when_mod_index_consumes_terminal_record() {
        // given
        let mut pdta = PdtaBytes::default();
        let mut pbag = bag_record(0, 0);
        pbag.extend_from_slice(&bag_record(0, 1));
        pdta.pbag = pbag;

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(
            result,
            Err(Error::IndexOutOfBounds {
                what: "preset zones",
                ..
            })
        ));
    }

    #[test]
    fn parse_should_reject_presets_when_bag_indices_decrease() {
        // given: two presets whose bag indices go 1 -> 0
        let mut pdta = PdtaBytes::default();
        let mut phdr = phdr_record("A", 0, 0, 1, 0);
        phdr.extend_from_slice(&phdr_record("B", 1, 0, 0, 0));
        phdr.extend_from_slice(&phdr_record("EOP", 0, 0, 1, 0));
        pdta.phdr = phdr;
        let mut pbag = bag_record(0, 0);
        pbag.extend_from_slice(&bag_record(0, 0));
        pdta.pbag = pbag;

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(
            result,
            Err(Error::IndexOrder("preset bag indices"))
        ));
    }

    #[test]
    fn parse_should_reject_zone_when_gen_index_exceeds_pgen_records() {
        // given: a zone whose sentinel bag points past the pgen terminal
        let mut pdta = PdtaBytes::default();
        let mut pbag = bag_record(0, 0);
        pbag.extend_from_slice(&bag_record(5, 0));
        pdta.pbag = pbag;

        // when
        let result = parse(&font_bytes(&pdta));

        // then
        assert!(matches!(
            result,
            Err(Error::IndexOutOfBounds {
                what: "preset zones",
                ..
            })
        ));
    }

    #[test]
    fn parse_should_capture_sm24_when_chunk_is_present() {
        // given
        let mut out = Vec::new();
        let riff_at = begin_container(&mut out, RIFF, *b"sfbk");
        let info_at = begin_container(&mut out, LIST, *b"INFO");
        push_chunk(&mut out, *b"ifil", &[2, 4, 0, 0]).unwrap();
        end_container(&mut out, info_at).unwrap();
        let sdta_at = begin_container(&mut out, LIST, *b"sdta");
        push_chunk(&mut out, *b"smpl", &[1, 2]).unwrap();
        push_chunk(&mut out, *b"sm24", &[3]).unwrap();
        end_container(&mut out, sdta_at).unwrap();
        let pdta_at = begin_container(&mut out, LIST, *b"pdta");
        let defaults = PdtaBytes::default();
        for (id, data) in [
            (*b"phdr", &defaults.phdr),
            (*b"pbag", &defaults.pbag),
            (*b"pmod", &defaults.pmod),
            (*b"pgen", &defaults.pgen),
            (*b"inst", &defaults.inst),
            (*b"ibag", &defaults.ibag),
            (*b"imod", &defaults.imod),
            (*b"igen", &defaults.igen),
            (*b"shdr", &defaults.shdr),
        ] {
            push_chunk(&mut out, id, data).unwrap();
        }
        end_container(&mut out, pdta_at).unwrap();
        end_container(&mut out, riff_at).unwrap();

        // when
        let font = parse(&out).unwrap();

        // then
        assert_eq!(font.sample_data, vec![1, 2]);
        assert_eq!(font.sample_data_24, Some(vec![3]));
    }
}
