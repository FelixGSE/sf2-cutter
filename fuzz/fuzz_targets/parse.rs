//! The parser must never panic, whatever bytes it is fed.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = sf2_cutter::parse::parse(data);
});
