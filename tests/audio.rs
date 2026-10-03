//! Renders an extracted font through fluidsynth and asserts the result is
//! audible — the strongest end-to-end check available headlessly. Skips when
//! fluidsynth is not on PATH.
#![allow(clippy::unwrap_used)] // the entire file is test code

use sf2_cutter::builder::{SoundFontBuilder, instrument_zone, sample_zone};
use sf2_cutter::extract::{Options, extract};
use sf2_cutter::select::Selection;
use sf2_cutter::write;

/// A loud ~220 Hz square wave at 44100 Hz; one second so the note sustains.
fn square_wave() -> Vec<i16> {
    (0..44_100)
        .map(|i| if (i / 100) % 2 == 0 { 16_000 } else { -16_000 })
        .collect()
}

/// A minimal type-0 Standard MIDI File: program 0, note-on C4, two beats,
/// note-off, end of track.
fn tiny_midi() -> Vec<u8> {
    let mut track: Vec<u8> = Vec::new();
    track.extend_from_slice(&[0x00, 0xC0, 0x00]); // program change to 0
    track.extend_from_slice(&[0x00, 0x90, 0x3C, 0x64]); // note on C4, vel 100
    track.extend_from_slice(&[0x81, 0x40, 0x80, 0x3C, 0x40]); // delta 192, note off
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]); // end of track

    let mut smf = b"MThd".to_vec();
    smf.extend_from_slice(&6u32.to_be_bytes());
    smf.extend_from_slice(&0u16.to_be_bytes()); // format 0
    smf.extend_from_slice(&1u16.to_be_bytes()); // one track
    smf.extend_from_slice(&96u16.to_be_bytes()); // ticks per quarter
    smf.extend_from_slice(b"MTrk");
    smf.extend_from_slice(&u32::try_from(track.len()).unwrap().to_be_bytes());
    smf.extend_from_slice(&track);
    smf
}

/// Peak absolute 16-bit amplitude of the `data` chunk in a WAV file.
fn wav_peak(bytes: &[u8]) -> i32 {
    // RIFF/WAVE: walk chunks after the 12-byte header to find `data`.
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            let data = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
            return data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| i32::from(i16::from_le_bytes(*c)).abs())
                .max()
                .unwrap_or(0);
        }
        pos += 8 + size + (size & 1);
    }
    0
}

#[test]
fn fluidsynth_should_render_audible_audio_when_playing_extracted_font() {
    // given: an extracted one-preset font with a loud square-wave sample
    if std::process::Command::new("fluidsynth")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: fluidsynth not on PATH");
        return;
    }
    let mut builder = SoundFontBuilder::new("Audio Test");
    let sample = builder
        .add_sample("square", &square_wave(), 44_100, 60)
        .unwrap();
    let instrument = builder
        .add_instrument("Square I", vec![sample_zone(sample)])
        .unwrap();
    builder.add_preset("Square Lead", 0, 0, vec![instrument_zone(instrument)]);
    let font = builder.build();
    let mut selection = Selection::new();
    selection.add_pattern("square");
    let result = extract(&font, &selection, &Options::default()).unwrap();

    let dir = std::env::temp_dir();
    let sf2_path = dir.join("sf2-cutter-audio-test.sf2");
    let midi_path = dir.join("sf2-cutter-audio-test.mid");
    let wav_path = dir.join("sf2-cutter-audio-test.wav");
    std::fs::write(&sf2_path, write::write(&result).unwrap()).unwrap();
    std::fs::write(&midi_path, tiny_midi()).unwrap();

    // when: fast-render the MIDI through the font into a WAV file
    let output = std::process::Command::new("fluidsynth")
        .args(["-ni", "-r", "44100", "-F"])
        .arg(&wav_path)
        .arg(&sf2_path)
        .arg(&midi_path)
        .output()
        .unwrap();

    // then: the rendered audio contains an actual signal, not silence
    let log = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        !log.contains("failed to load"),
        "fluidsynth rejected the font: {log}"
    );
    let wav = std::fs::read(&wav_path).unwrap();
    let peak = wav_peak(&wav);
    assert!(
        peak > 500,
        "rendered audio is (nearly) silent: peak amplitude {peak}"
    );
}
