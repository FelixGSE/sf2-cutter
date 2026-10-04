//! Convert fonts between plain sf2 and SF3 (Ogg-Vorbis-compressed) form.
//!
//! Compression is lossy and drops any 24-bit (`sm24`) extension; stereo pairs
//! stay two linked mono streams, matching the `MuseScore` convention. Both
//! directions keep presets, instruments, zones, and sample order untouched —
//! only the sample *data* representation and the header offsets change.

use crate::error::Error;
use crate::model::{SAMPLE_TYPE_COMPRESSED, SoundFont};

/// Lossily compresses every RAM sample to an Ogg-Vorbis stream.
///
/// `quality` is the Vorbis VBR quality, `0.0..=1.0`. Already-compressed
/// samples are carried over verbatim (re-based); ROM headers pass through.
/// The output declares `ifil` 3.0 and is provenance-stamped like extraction.
///
/// # Errors
///
/// Returns [`Error::Sf3Encode`] when the encoder rejects a sample (e.g.
/// out-of-range quality), [`Error::IndexOutOfBounds`] for out-of-range sample
/// offsets, or [`Error::TooLarge`] when streams exceed 32-bit offsets.
#[cfg(feature = "sf3-write")]
pub fn compress(font: &SoundFont, quality: f32) -> Result<SoundFont, Error> {
    let mut out = SoundFont {
        info: crate::extract::stamped_info(&font.info, None),
        sample_data: Vec::new(),
        sample_data_24: None,
        presets: font.presets.clone(),
        instruments: font.instruments.clone(),
        samples: Vec::with_capacity(font.samples.len()),
    };
    set_ifil(&mut out, [3, 0, 0, 0]);
    for sample in &font.samples {
        let mut header = sample.clone();
        if !sample.is_rom() {
            let cursor =
                u32::try_from(out.sample_data.len()).map_err(|_| Error::TooLarge("smpl"))?;
            let stream = if sample.is_compressed() {
                crate::sf3::compressed_stream(font, sample, false)?.to_vec()
            } else {
                let pcm = crate::extract::plain_sample_bytes(font, sample)?;
                let frames = u32::try_from(pcm.len() / 2).map_err(|_| Error::TooLarge("sample"))?;
                // SF3 loop points are relative to the decoded sample; clamp
                // them into it so players reading the sf3 directly never see
                // a loop outside the audio.
                header.start_loop = sample.start_loop.saturating_sub(sample.start).min(frames);
                header.end_loop = sample
                    .end_loop
                    .saturating_sub(sample.start)
                    .clamp(header.start_loop, frames);
                header.sample_type |= SAMPLE_TYPE_COMPRESSED;
                if pcm.is_empty() {
                    // No stream for an empty sample; decode_sample maps an
                    // empty range back to no audio.
                    Vec::new()
                } else {
                    crate::sf3::encode_ogg(
                        &sample.name.to_display(),
                        pcm,
                        sample.sample_rate,
                        quality,
                    )?
                }
            };
            let len = u32::try_from(stream.len()).map_err(|_| Error::TooLarge("smpl"))?;
            out.sample_data.extend_from_slice(&stream);
            header.start = cursor;
            header.end = cursor.checked_add(len).ok_or(Error::TooLarge("smpl"))?;
        }
        out.samples.push(header);
    }
    Ok(out)
}

/// Decodes every compressed sample back to plain PCM.
///
/// Unlike extraction this keeps *everything* — no reachability pruning — so
/// it is a pure format conversion. The output declares `ifil` 2.04.
///
/// # Errors
///
/// Returns [`Error::Sf3Decode`] for undecodable streams and
/// [`Error::IndexOutOfBounds`] for out-of-range sample offsets.
pub fn decompress(font: &SoundFont) -> Result<SoundFont, Error> {
    let mut data = Vec::new();
    let mut data_24 = font.sm24_usable().then(Vec::new);
    let mut samples = Vec::with_capacity(font.samples.len());
    for sample in &font.samples {
        let mut header = sample.clone();
        if sample.is_rom() {
            // ROM offsets address ROM, not smpl; nothing to relocate.
        } else if sample.is_compressed() {
            let pcm = crate::sf3::decode_sample(font, sample, false)?;
            crate::extract::place_decoded(sample, &mut header, &pcm, &mut data, data_24.as_mut())?;
        } else {
            crate::extract::relocate_sample(
                font,
                sample,
                &mut header,
                &mut data,
                data_24.as_mut(),
                false,
            )?;
        }
        samples.push(header);
    }
    let mut out = SoundFont {
        info: crate::extract::stamped_info(&font.info, None),
        sample_data: data,
        sample_data_24: data_24,
        presets: font.presets.clone(),
        instruments: font.instruments.clone(),
        samples,
    };
    set_ifil(&mut out, [2, 0, 4, 0]);
    Ok(out)
}

fn set_ifil(font: &mut SoundFont, bytes: [u8; 4]) {
    if let Some(chunk) = font.info.iter_mut().find(|c| c.id == *b"ifil") {
        chunk.data = bytes.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::test_font;
    use crate::validate::{has_errors, validate};

    #[test]
    fn decompress_should_keep_model_identical_when_font_is_plain() {
        // given: a plain 2.04 font (nothing to decode)
        let font = test_font();

        // when
        let result = decompress(&font).unwrap();

        // then: structure and audio byte-identical; only ISFT stamping differs
        assert_eq!(result.presets, font.presets);
        assert_eq!(result.instruments, font.instruments);
        assert_eq!(result.samples, font.samples);
        assert_eq!(result.sample_data, font.sample_data);
        assert_eq!(result.version(), Some((2, 4)));
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn compress_should_produce_valid_sf3_when_font_is_plain() {
        // given
        let font = test_font();

        // when
        let sf3 = compress(&font, 0.5).unwrap();

        // then: version 3, every RAM sample compressed with byte offsets in
        // range, loops decoded-relative, and the model survives write/parse
        assert_eq!(sf3.version(), Some((3, 0)));
        assert!(sf3.samples.iter().all(|s| s.is_rom() || s.is_compressed()));
        for sample in &sf3.samples {
            assert!(sample.end as usize <= sf3.sample_data.len());
        }
        assert_eq!(sf3.samples[0].start_loop, 0);
        assert_eq!(sf3.samples[0].end_loop, 200);
        assert!(!has_errors(&validate(&sf3)));
        let reparsed = crate::parse::parse(&crate::write::write(&sf3).unwrap()).unwrap();
        assert_eq!(reparsed, sf3);
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn round_trip_should_preserve_structure_when_compressing_then_decompressing() {
        // given
        let font = test_font();

        // when
        let back = decompress(&compress(&font, 0.6).unwrap()).unwrap();

        // then: zones untouched, sample counts/lengths/loops preserved
        assert_eq!(back.presets, font.presets);
        assert_eq!(back.instruments.len(), font.instruments.len());
        assert_eq!(back.samples.len(), font.samples.len());
        for (original, decoded) in font.samples.iter().zip(&back.samples) {
            assert_eq!(decoded.name, original.name);
            assert_eq!(decoded.len_points(), original.len_points());
            assert_eq!(
                decoded.start_loop - decoded.start,
                original.start_loop - original.start
            );
            assert_eq!(decoded.sample_link, original.sample_link);
            assert!(!decoded.is_compressed());
        }
        assert_eq!(back.version(), Some((2, 4)));
        assert!(!has_errors(&validate(&back)));
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn compress_should_carry_streams_over_verbatim_when_font_is_already_sf3() {
        // given
        let once = compress(&test_font(), 0.5).unwrap();

        // when
        let twice = compress(&once, 0.9).unwrap();

        // then: identical headers and byte-identical Ogg streams (no re-encode)
        assert_eq!(twice.samples, once.samples);
        assert_eq!(twice.sample_data, once.sample_data);
        assert_eq!(twice.version(), Some((3, 0)));
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn round_trip_should_keep_empty_sample_empty_when_compressing() {
        // given: sample 0 shortened to zero points
        let mut font = test_font();
        font.samples[0].end = font.samples[0].start;
        font.samples[0].start_loop = font.samples[0].start;
        font.samples[0].end_loop = font.samples[0].start;

        // when
        let sf3 = compress(&font, 0.5).unwrap();
        let back = decompress(&sf3).unwrap();

        // then: no stream stored, and still zero points after decoding
        assert_eq!(sf3.samples[0].start, sf3.samples[0].end);
        assert_eq!(back.samples[0].len_points(), 0);
        assert_eq!(back.samples[1].len_points(), font.samples[1].len_points());
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn compress_should_clamp_loop_points_when_they_lie_outside_the_sample() {
        // given: loop starting before and ending past the 200-point sample
        let mut font = test_font();
        let start = font.samples[0].start;
        font.samples[0].start = start + 10;
        font.samples[0].start_loop = start;
        font.samples[0].end_loop = font.samples[0].end + 50;

        // when
        let sf3 = compress(&font, 0.5).unwrap();

        // then: decoded-relative loop inside 0..=190
        assert_eq!(sf3.samples[0].start_loop, 0);
        assert_eq!(sf3.samples[0].end_loop, 190);
    }

    #[cfg(feature = "sf3-write")]
    #[test]
    fn compress_should_fail_when_quality_is_out_of_range() {
        // given / when
        let result = compress(&test_font(), 42.0);

        // then
        assert!(matches!(result, Err(Error::Sf3Encode { .. })));
    }
}
