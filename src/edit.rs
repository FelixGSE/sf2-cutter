//! In-place edits on a [`SoundFont`]: move and rename presets.
//!
//! Used by the CLI after extraction or merging, but the functions address the
//! font directly, so they compose with anything.

use std::collections::HashMap;

use crate::error::Error;
use crate::model::{FixedName, Preset, SoundFont};
use crate::select::PresetSpec;

/// The `bank:program` address of a preset.
const fn address_of(preset: &Preset) -> PresetSpec {
    PresetSpec {
        bank: preset.bank,
        program: preset.program,
    }
}

/// Applies all moves simultaneously (so `0:0=0:1` plus `0:1=0:0` swaps).
///
/// Every source address must exist, no source may appear twice, and no move
/// may land on an address that is occupied after all moves are applied.
///
/// # Errors
///
/// Returns [`Error::PresetNotFound`] for an unmatched source,
/// [`Error::DuplicateMoveSource`] when two moves share a source, or
/// [`Error::MoveCollision`] when a target address ends up contested.
pub fn remap_presets(
    font: &mut SoundFont,
    moves: &[(PresetSpec, PresetSpec)],
) -> Result<(), Error> {
    let mut by_source: HashMap<PresetSpec, PresetSpec> = HashMap::new();
    for (from, to) in moves {
        if by_source.insert(*from, *to).is_some() {
            return Err(Error::DuplicateMoveSource {
                bank: from.bank,
                program: from.program,
            });
        }
    }

    let mut was_moved = vec![false; font.presets.len()];
    for (from, _) in moves {
        let mut found = false;
        for (index, preset) in font.presets.iter().enumerate() {
            if preset.bank == from.bank && preset.program == from.program {
                was_moved[index] = true;
                found = true;
            }
        }
        if !found {
            return Err(Error::PresetNotFound {
                bank: from.bank,
                program: from.program,
            });
        }
    }

    let targets: Vec<Option<PresetSpec>> = font
        .presets
        .iter()
        .map(|preset| by_source.get(&address_of(preset)).copied())
        .collect();
    for (preset, target) in font.presets.iter_mut().zip(&targets) {
        if let Some(target) = target {
            preset.bank = target.bank;
            preset.program = target.program;
        }
    }

    let mut occupancy: HashMap<PresetSpec, usize> = HashMap::new();
    for preset in &font.presets {
        *occupancy.entry(address_of(preset)).or_insert(0) += 1;
    }
    for (index, preset) in font.presets.iter().enumerate() {
        if was_moved[index] && occupancy[&address_of(preset)] > 1 {
            return Err(Error::MoveCollision {
                bank: preset.bank,
                program: preset.program,
            });
        }
    }
    Ok(())
}

/// Renames every preset at the given address (names longer than 19 bytes are
/// truncated, as the on-disk field requires).
///
/// # Errors
///
/// Returns [`Error::PresetNotFound`] when nothing sits at the address.
pub fn rename_preset(font: &mut SoundFont, spec: PresetSpec, name: &str) -> Result<(), Error> {
    let mut found = false;
    for preset in &mut font.presets {
        if preset.bank == spec.bank && preset.program == spec.program {
            preset.name = FixedName::from_text(name);
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(Error::PresetNotFound {
            bank: spec.bank,
            program: spec.program,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::test_font;

    fn spec(bank: u16, program: u16) -> PresetSpec {
        PresetSpec { bank, program }
    }

    fn address_of(font: &SoundFont, name: &str) -> (u16, u16) {
        let preset = font
            .presets
            .iter()
            .find(|p| p.name.to_display() == name)
            .unwrap();
        (preset.bank, preset.program)
    }

    #[test]
    fn remap_should_move_preset_when_target_is_free() {
        // given: fixture has Bright Piano at 0:0, Slow Strings at 0:48
        let mut font = test_font();

        // when
        remap_presets(&mut font, &[(spec(0, 0), spec(5, 20))]).unwrap();

        // then
        assert_eq!(address_of(&font, "Bright Piano"), (5, 20));
        assert_eq!(address_of(&font, "Slow Strings"), (0, 48));
    }

    #[test]
    fn remap_should_swap_presets_when_moves_cross() {
        // given
        let mut font = test_font();

        // when: simultaneous application makes a swap possible
        remap_presets(
            &mut font,
            &[(spec(0, 0), spec(0, 48)), (spec(0, 48), spec(0, 0))],
        )
        .unwrap();

        // then
        assert_eq!(address_of(&font, "Bright Piano"), (0, 48));
        assert_eq!(address_of(&font, "Slow Strings"), (0, 0));
    }

    #[test]
    fn remap_should_fail_when_source_is_missing() {
        // given
        let mut font = test_font();

        // when
        let result = remap_presets(&mut font, &[(spec(9, 9), spec(0, 1))]);

        // then
        assert!(matches!(
            result,
            Err(Error::PresetNotFound {
                bank: 9,
                program: 9
            })
        ));
    }

    #[test]
    fn remap_should_fail_when_target_collides_with_unmoved_preset() {
        // given: 0:48 stays put, 0:0 is moved onto it
        let mut font = test_font();

        // when
        let result = remap_presets(&mut font, &[(spec(0, 0), spec(0, 48))]);

        // then
        assert!(matches!(
            result,
            Err(Error::MoveCollision {
                bank: 0,
                program: 48
            })
        ));
    }

    #[test]
    fn remap_should_fail_when_duplicate_source_given() {
        // given
        let mut font = test_font();

        // when
        let result = remap_presets(
            &mut font,
            &[(spec(0, 0), spec(0, 1)), (spec(0, 0), spec(0, 2))],
        );

        // then
        assert!(matches!(result, Err(Error::DuplicateMoveSource { .. })));
    }

    #[test]
    fn rename_should_change_name_when_preset_exists() {
        // given
        let mut font = test_font();

        // when
        rename_preset(&mut font, spec(0, 0), "Concert Grand").unwrap();

        // then: only the addressed preset changes
        assert_eq!(font.presets[0].name.to_display(), "Concert Grand");
        assert_eq!(font.presets[1].name.to_display(), "Slow Strings");
        assert_eq!(font.presets[2].name.to_display(), "Standard Kit");
    }

    #[test]
    fn remap_should_ignore_presets_sharing_only_bank_or_program_when_matching() {
        // given: duplicates at 0:5 that share the bank of the move source
        let mut font = test_font();
        font.presets[1].bank = 0;
        font.presets[1].program = 5;
        font.presets[2].bank = 0;
        font.presets[2].program = 5;

        // when: moving 0:0 must not treat the 0:5 duplicates as moved
        remap_presets(&mut font, &[(spec(0, 0), spec(9, 9))]).unwrap();

        // then
        assert_eq!(address_of(&font, "Bright Piano"), (9, 9));
        assert_eq!((font.presets[1].bank, font.presets[1].program), (0, 5));
        assert_eq!((font.presets[2].bank, font.presets[2].program), (0, 5));
    }

    #[test]
    fn rename_should_truncate_name_when_longer_than_field() {
        // given
        let mut font = test_font();

        // when
        rename_preset(&mut font, spec(0, 0), "An Extremely Long Preset Name").unwrap();

        // then: 19 bytes plus NUL terminator
        assert_eq!(font.presets[0].name.to_display(), "An Extremely Long P");
    }

    #[test]
    fn rename_should_fail_when_preset_is_missing() {
        // given
        let mut font = test_font();

        // when
        let result = rename_preset(&mut font, spec(77, 7), "Ghost");

        // then
        assert!(matches!(result, Err(Error::PresetNotFound { .. })));
    }
}
