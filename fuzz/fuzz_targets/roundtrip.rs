//! Everything the parser accepts must re-serialise and re-parse to an equal
//! model. The writer may reject fonts whose record counts exceed 16-bit SF2
//! indices (such inputs are unaddressable, so not round-trippable by design),
//! and it normalises odd-length `sm24` payloads by padding them to even.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(font) = sf2_cutter::parse::parse(data) else {
        return;
    };
    match sf2_cutter::write::write(&font) {
        Err(sf2_cutter::Error::TooManyRecords(_) | sf2_cutter::Error::TooLarge(_)) => {}
        Err(other) => panic!("writer rejected a parsed font: {other}"),
        Ok(bytes) => {
            let again = sf2_cutter::parse::parse(&bytes).expect("reparse of written font failed");
            let mut expected = font;
            if let Some(data_24) = &mut expected.sample_data_24 {
                if data_24.len() % 2 == 1 {
                    data_24.push(0); // the writer pads sm24 to an even size
                }
            }
            assert_eq!(again, expected, "model changed across write/parse");
        }
    }
});
