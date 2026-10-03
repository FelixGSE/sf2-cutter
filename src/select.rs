//! Decide which presets to keep: name patterns, `BANK:PROG` specs, explicit
//! indices, TOML recipes, and the drum-kit convenience flag, all combined as
//! a union.

use std::collections::BTreeSet;
use std::str::FromStr;

use serde::Deserialize;

use crate::error::Error;
use crate::model::Preset;

/// The GM percussion bank kept by `keep_drums`.
pub const DRUM_BANK: u16 = 128;

/// A `BANK:PROG` preset address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetSpec {
    /// MIDI bank number.
    pub bank: u16,
    /// MIDI program number.
    pub program: u16,
}

impl FromStr for PresetSpec {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidPresetSpec(text.to_string());
        let (bank, program) = text.split_once(':').ok_or_else(invalid)?;
        Ok(Self {
            bank: bank.trim().parse().map_err(|_| invalid())?,
            program: program.trim().parse().map_err(|_| invalid())?,
        })
    }
}

/// Union of selection criteria; a preset is kept when any criterion matches.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    patterns: Vec<String>,
    specs: Vec<PresetSpec>,
    indices: BTreeSet<usize>,
    keep_drums: bool,
}

impl Selection {
    /// An empty selection that matches nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a case-insensitive name pattern. Without `*` it matches as a
    /// substring; with `*` wildcards it must match the whole name. Blank
    /// patterns are ignored (an empty substring would match every preset).
    pub fn add_pattern(&mut self, pattern: &str) {
        if pattern.trim().is_empty() {
            return;
        }
        self.patterns.push(pattern.to_lowercase());
    }

    /// Adds a `BANK:PROG` address.
    pub fn add_spec(&mut self, spec: PresetSpec) {
        self.specs.push(spec);
    }

    /// Adds an explicit preset index (position in the font's preset list).
    pub fn add_index(&mut self, index: usize) {
        self.indices.insert(index);
    }

    /// Keep every preset on the percussion bank ([`DRUM_BANK`]).
    pub fn set_keep_drums(&mut self, keep: bool) {
        self.keep_drums = keep;
    }

    /// Merges the criteria of a recipe into this selection.
    ///
    /// Note: [`Recipe::renumber`] is not a selection criterion; copy it into
    /// [`crate::extract::Options`] yourself (as the CLI does).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPresetSpec`] when a `presets` entry is not a
    /// valid `BANK:PROG` address.
    pub fn apply_recipe(&mut self, recipe: &Recipe) -> Result<(), Error> {
        for pattern in &recipe.patterns {
            self.add_pattern(pattern);
        }
        for spec in &recipe.presets {
            self.add_spec(spec.parse()?);
        }
        if recipe.keep_drums {
            self.set_keep_drums(true);
        }
        Ok(())
    }

    /// Whether no criterion has been added at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
            && self.specs.is_empty()
            && self.indices.is_empty()
            && !self.keep_drums
    }

    /// Whether the preset at `index` is selected.
    #[must_use]
    pub fn matches(&self, index: usize, preset: &Preset) -> bool {
        if self.indices.contains(&index) {
            return true;
        }
        if self.keep_drums && preset.bank == DRUM_BANK {
            return true;
        }
        if self
            .specs
            .iter()
            .any(|s| s.bank == preset.bank && s.program == preset.program)
        {
            return true;
        }
        let name = preset.name.to_display().to_lowercase();
        self.patterns.iter().any(|p| name_matches(p, &name))
    }
}

/// Matches a lowercased pattern against a lowercased name: substring match
/// without `*`, anchored wildcard match with `*`.
fn name_matches(pattern: &str, name: &str) -> bool {
    if pattern.contains('*') {
        let pattern: Vec<char> = pattern.chars().collect();
        let name: Vec<char> = name.chars().collect();
        wildcard_match(&pattern, &name)
    } else {
        name.contains(pattern)
    }
}

/// Iterative `*`-wildcard matcher with backtracking over the last star.
fn wildcard_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        if pattern.get(p) == Some(&'*') {
            backtrack = Some((p, t));
            p += 1;
        } else if pattern.get(p) == Some(&text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = backtrack {
            backtrack = Some((star_p, star_t + 1));
            p = star_p + 1;
            t = star_t + 1;
        } else {
            return false;
        }
    }
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}

/// A TOML extraction recipe:
///
/// ```toml
/// match = ["*piano*", "rhodes"]
/// presets = ["0:0", "0:4"]
/// keep_drums = true
/// renumber = false
/// ```
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Name patterns (TOML key `match`).
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,
    /// `BANK:PROG` addresses as strings.
    #[serde(default)]
    pub presets: Vec<String>,
    /// Keep all percussion-bank presets.
    #[serde(default)]
    pub keep_drums: bool,
    /// Compact program numbers in the output. Not consumed by
    /// [`Selection::apply_recipe`]; callers feed it into
    /// [`crate::extract::Options::renumber`].
    #[serde(default)]
    pub renumber: bool,
}

impl Recipe {
    /// Parses a recipe from TOML text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRecipe`] when the TOML is malformed or has
    /// unknown fields.
    pub fn from_toml_str(text: &str) -> Result<Self, Error> {
        toml::from_str(text).map_err(|e| Error::InvalidRecipe(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FixedName;

    fn preset(name: &str, bank: u16, program: u16) -> Preset {
        Preset {
            name: FixedName::from_text(name),
            program,
            bank,
            library: 0,
            genre: 0,
            morphology: 0,
            zones: vec![],
        }
    }

    #[test]
    fn preset_spec_should_parse_bank_and_program_when_text_is_valid() {
        // given
        let text = "128: 7";

        // when
        let spec: PresetSpec = text.parse().unwrap();

        // then
        assert_eq!(
            spec,
            PresetSpec {
                bank: 128,
                program: 7
            }
        );
    }

    #[test]
    fn preset_spec_should_fail_when_separator_is_missing() {
        // given
        let text = "42";

        // when
        let result: Result<PresetSpec, _> = text.parse();

        // then
        assert!(matches!(result, Err(Error::InvalidPresetSpec(_))));
    }

    #[test]
    fn preset_spec_should_fail_when_number_is_not_numeric() {
        // given
        let text = "0:piano";

        // when
        let result: Result<PresetSpec, _> = text.parse();

        // then
        assert!(matches!(result, Err(Error::InvalidPresetSpec(_))));
    }

    #[test]
    fn selection_should_match_preset_when_pattern_is_substring() {
        // given
        let mut selection = Selection::new();
        selection.add_pattern("PIANO");

        // when / then
        assert!(selection.matches(0, &preset("Bright Piano", 0, 1)));
        assert!(!selection.matches(1, &preset("Violin", 0, 40)));
    }

    #[test]
    fn selection_should_match_preset_when_wildcard_pattern_matches_whole_name() {
        // given
        let mut selection = Selection::new();
        selection.add_pattern("ya*ha gr*");

        // when / then
        assert!(selection.matches(0, &preset("Yamaha Grand", 0, 0)));
        assert!(!selection.matches(1, &preset("A Yamaha Grand", 0, 1)));
    }

    #[test]
    fn selection_should_match_preset_when_spec_addresses_it() {
        // given
        let mut selection = Selection::new();
        selection.add_spec(PresetSpec {
            bank: 8,
            program: 14,
        });

        // when / then
        assert!(selection.matches(0, &preset("Church Bell", 8, 14)));
        assert!(!selection.matches(1, &preset("Church Bell", 0, 14)));
    }

    #[test]
    fn selection_should_match_drum_presets_when_keep_drums_is_set() {
        // given
        let mut selection = Selection::new();
        selection.set_keep_drums(true);

        // when / then
        assert!(selection.matches(0, &preset("Standard Kit", 128, 0)));
        assert!(!selection.matches(1, &preset("Piano", 0, 0)));
    }

    #[test]
    fn selection_should_match_preset_when_index_added_explicitly() {
        // given
        let mut selection = Selection::new();
        selection.add_index(3);

        // when / then
        assert!(selection.matches(3, &preset("Anything", 5, 5)));
        assert!(!selection.matches(2, &preset("Anything", 5, 5)));
    }

    #[test]
    fn selection_should_ignore_pattern_when_blank() {
        // given
        let mut selection = Selection::new();

        // when
        selection.add_pattern("   ");
        selection.add_pattern("");

        // then: blank patterns are not criteria and match nothing
        assert!(selection.is_empty());
        assert!(!selection.matches(0, &preset("Piano", 0, 0)));
    }

    #[test]
    fn selection_should_report_empty_when_no_criteria_added() {
        // given
        let selection = Selection::new();

        // when / then
        assert!(selection.is_empty());
        assert!(!selection.matches(0, &preset("Piano", 0, 0)));
    }

    #[test]
    fn selection_should_report_not_empty_when_any_single_criterion_added() {
        // given / when / then: each criterion alone makes the selection non-empty
        let mut by_pattern = Selection::new();
        by_pattern.add_pattern("piano");
        assert!(!by_pattern.is_empty());

        let mut by_spec = Selection::new();
        by_spec.add_spec(PresetSpec {
            bank: 0,
            program: 0,
        });
        assert!(!by_spec.is_empty());

        let mut by_index = Selection::new();
        by_index.add_index(0);
        assert!(!by_index.is_empty());

        let mut by_drums = Selection::new();
        by_drums.set_keep_drums(true);
        assert!(!by_drums.is_empty());
    }

    #[test]
    fn recipe_should_populate_selection_when_toml_is_valid() {
        // given
        let toml_text = r#"
            match = ["*piano*"]
            presets = ["128:0"]
            keep_drums = true
            renumber = true
        "#;

        // when
        let recipe = Recipe::from_toml_str(toml_text).unwrap();
        let mut selection = Selection::new();
        selection.apply_recipe(&recipe).unwrap();

        // then
        assert!(recipe.renumber);
        assert!(selection.matches(0, &preset("Grand Piano", 0, 0)));
        assert!(selection.matches(1, &preset("Room Kit", 128, 8)));
    }

    #[test]
    fn recipe_should_fail_when_toml_has_unknown_field() {
        // given
        let toml_text = "matcj = [\"typo\"]";

        // when
        let result = Recipe::from_toml_str(toml_text);

        // then
        assert!(matches!(result, Err(Error::InvalidRecipe(_))));
    }

    #[test]
    fn recipe_should_fail_when_preset_spec_is_malformed() {
        // given
        let recipe = Recipe {
            presets: vec!["not-a-spec".into()],
            ..Recipe::default()
        };
        let mut selection = Selection::new();

        // when
        let result = selection.apply_recipe(&recipe);

        // then
        assert!(matches!(result, Err(Error::InvalidPresetSpec(_))));
    }

    #[test]
    fn wildcard_should_match_text_when_stars_span_gaps() {
        // given / when / then
        assert!(wildcard_match(
            &"a*c".chars().collect::<Vec<_>>(),
            &"abbbc".chars().collect::<Vec<_>>()
        ));
        assert!(wildcard_match(
            &"*end".chars().collect::<Vec<_>>(),
            &"the end".chars().collect::<Vec<_>>()
        ));
        assert!(wildcard_match(
            &"start*".chars().collect::<Vec<_>>(),
            &"start of it".chars().collect::<Vec<_>>()
        ));
        assert!(!wildcard_match(
            &"a*c".chars().collect::<Vec<_>>(),
            &"abd".chars().collect::<Vec<_>>()
        ));
        assert!(wildcard_match(
            &"a*b*c".chars().collect::<Vec<_>>(),
            &"aXbYbZc".chars().collect::<Vec<_>>()
        ));
        assert!(wildcard_match(
            &"ab*cd".chars().collect::<Vec<_>>(),
            &"abXcYcd".chars().collect::<Vec<_>>()
        ));
        assert!(!wildcard_match(
            &"a*bc".chars().collect::<Vec<_>>(),
            &"abXd".chars().collect::<Vec<_>>()
        ));
        assert!(wildcard_match(&"*".chars().collect::<Vec<_>>(), &[]));
    }
}
