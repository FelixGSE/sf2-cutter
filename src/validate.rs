//! Integrity checks on a parsed [`SoundFont`] model.
//!
//! [`validate`] never fails; it returns a list of [`Issue`]s. Errors are
//! violations that break playback or extraction (dangling references, sample
//! data out of range); warnings are spec deviations most synthesisers
//! tolerate (odd loop points, missing optional `INFO` chunks).

use std::fmt;

use crate::model::{GEN_INSTRUMENT, GEN_SAMPLE_ID, SoundFont};

/// How severe an [`Issue`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Breaks playback or extraction.
    Error,
    /// Spec deviation that synthesisers usually tolerate.
    Warning,
}

/// A single finding produced by [`validate`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Issue {
    /// Severity of the finding.
    pub severity: Severity,
    /// Human-readable description.
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{tag}: {}", self.message)
    }
}

/// Runs all integrity checks and returns the findings.
#[must_use]
pub fn validate(font: &SoundFont) -> Vec<Issue> {
    let mut issues = Vec::new();
    check_info(font, &mut issues);
    check_presets(font, &mut issues);
    check_instruments(font, &mut issues);
    check_samples(font, &mut issues);
    check_sm24(font, &mut issues);
    issues
}

/// Whether any finding is an [`Severity::Error`].
#[must_use]
pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

fn error(issues: &mut Vec<Issue>, message: String) {
    issues.push(Issue {
        severity: Severity::Error,
        message,
    });
}

fn warning(issues: &mut Vec<Issue>, message: String) {
    issues.push(Issue {
        severity: Severity::Warning,
        message,
    });
}

fn check_info(font: &SoundFont, issues: &mut Vec<Issue>) {
    match font.version() {
        None => error(issues, "missing or malformed `ifil` version chunk".into()),
        Some((major, _)) if major != 2 => {
            warning(
                issues,
                format!("unexpected SoundFont major version {major}"),
            );
        }
        Some(_) => {}
    }
    for (id, label) in [(*b"INAM", "INAM (bank name)"), (*b"isng", "isng (engine)")] {
        if font.info_chunk(id).is_none() {
            warning(issues, format!("missing recommended INFO chunk `{label}`"));
        }
    }
    if font.presets.is_empty() {
        warning(issues, "font contains no presets".into());
    }
}

fn check_sm24(font: &SoundFont, issues: &mut Vec<Issue>) {
    if font.sample_data_24.is_some() && !font.sm24_usable() {
        warning(
            issues,
            "sm24 chunk is unusable (version < 2.04 or size mismatch) and will be ignored".into(),
        );
    }
}

fn check_presets(font: &SoundFont, issues: &mut Vec<Issue>) {
    let mut seen = std::collections::HashSet::new();
    for (index, preset) in font.presets.iter().enumerate() {
        let label = format!("preset {index} `{}`", preset.name);
        if !seen.insert((preset.bank, preset.program)) {
            warning(
                issues,
                format!(
                    "duplicate bank:program {}:{} at {label}",
                    preset.bank, preset.program
                ),
            );
        }
        if preset.bank > 128 {
            warning(issues, format!("{label}: bank {} exceeds 128", preset.bank));
        }
        if preset.program > 127 {
            warning(
                issues,
                format!("{label}: program {} exceeds 127", preset.program),
            );
        }
        check_zone_refs(
            &preset.zones,
            GEN_INSTRUMENT,
            font.instruments.len(),
            &label,
            "instrument",
            issues,
        );
    }
}

fn check_instruments(font: &SoundFont, issues: &mut Vec<Issue>) {
    for (index, instrument) in font.instruments.iter().enumerate() {
        let label = format!("instrument {index} `{}`", instrument.name);
        check_zone_refs(
            &instrument.zones,
            GEN_SAMPLE_ID,
            font.samples.len(),
            &label,
            "sample",
            issues,
        );
    }
}

/// Checks the reference generators of a zone list: every zone except a single
/// leading global zone must reference a valid target, and the reference must
/// be the final generator of its zone.
fn check_zone_refs(
    zones: &[crate::model::Zone],
    ref_oper: u16,
    target_count: usize,
    owner: &str,
    target_kind: &str,
    issues: &mut Vec<Issue>,
) {
    for (zone_index, zone) in zones.iter().enumerate() {
        let reference = zone.gens.iter().rev().find(|g| g.oper == ref_oper);
        match reference {
            None if zone_index == 0 => {} // global zone
            None => warning(
                issues,
                format!("{owner} zone {zone_index} has no {target_kind} reference and is ignored"),
            ),
            Some(generator) => {
                let ref_count = zone.gens.iter().filter(|g| g.oper == ref_oper).count();
                if ref_count > 1 {
                    warning(
                        issues,
                        format!(
                            "{owner} zone {zone_index} has {ref_count} {target_kind} references; only the last is used"
                        ),
                    );
                }
                if usize::from(generator.amount) >= target_count {
                    error(
                        issues,
                        format!(
                            "{owner} zone {zone_index} references {target_kind} {} but only {target_count} exist",
                            generator.amount
                        ),
                    );
                }
                if zone.gens.last().map(|g| g.oper) != Some(ref_oper) {
                    warning(
                        issues,
                        format!(
                            "{owner} zone {zone_index}: {target_kind} reference is not the final generator"
                        ),
                    );
                }
            }
        }
    }
}

fn check_samples(font: &SoundFont, issues: &mut Vec<Issue>) {
    let total_points = font.sample_points() as u64;
    for (index, sample) in font.samples.iter().enumerate() {
        let label = format!("sample {index} `{}`", sample.name);
        if sample.is_rom() {
            continue;
        }
        check_sample_range(sample, total_points, &label, issues);
        check_sample_link(font, index, sample, &label, issues);
    }
}

fn check_sample_range(
    sample: &crate::model::SampleHeader,
    total_points: u64,
    label: &str,
    issues: &mut Vec<Issue>,
) {
    if u64::from(sample.end) > total_points {
        error(
            issues,
            format!(
                "{label}: end point {} exceeds sample data ({total_points} points)",
                sample.end
            ),
        );
    }
    if sample.start > sample.end {
        error(
            issues,
            format!(
                "{label}: start point {} is after end point {}",
                sample.start, sample.end
            ),
        );
    } else if sample.len_points() < 48 {
        warning(
            issues,
            format!(
                "{label}: only {} sample points (spec minimum is 48)",
                sample.len_points()
            ),
        );
    }
    let loop_ok = sample.start <= sample.start_loop
        && sample.start_loop <= sample.end_loop
        && sample.end_loop <= sample.end;
    if !loop_ok {
        warning(
            issues,
            format!(
                "{label}: loop points {}..{} not within sample {}..{}",
                sample.start_loop, sample.end_loop, sample.start, sample.end
            ),
        );
    }
}

/// A linked-type sample whose partner does not link back (or does not exist)
/// carries a broken link; extraction treats it as unlinked, so it is reported
/// as a warning rather than an error.
fn check_sample_link(
    font: &SoundFont,
    index: usize,
    sample: &crate::model::SampleHeader,
    label: &str,
    issues: &mut Vec<Issue>,
) {
    if sample.is_linked() && font.mutual_link(index).is_none() {
        warning(
            issues,
            format!(
                "{label}: link to sample {} is not mutual; treated as unlinked",
                sample.sample_link
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::test_font;
    use crate::model::{GEN_INSTRUMENT, Generator, Zone};

    #[test]
    fn validate_should_return_no_issues_when_font_is_well_formed() {
        // given
        let font = test_font();

        // when
        let issues = validate(&font);

        // then
        assert_eq!(issues, Vec::new());
    }

    #[test]
    fn validate_should_report_error_when_preset_references_missing_instrument() {
        // given
        let mut font = test_font();
        font.presets[0].zones[0] = Zone {
            gens: vec![Generator {
                oper: GEN_INSTRUMENT,
                amount: 99,
            }],
            mods: vec![],
        };

        // when
        let issues = validate(&font);

        // then
        assert!(has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("references instrument 99"))
        );
    }

    #[test]
    fn validate_should_report_error_when_sample_end_exceeds_data() {
        // given
        let mut font = test_font();
        font.samples[0].end = u32::MAX;

        // when
        let issues = validate(&font);

        // then
        assert!(has_errors(&issues));
    }

    #[test]
    fn validate_should_report_error_when_ifil_is_missing() {
        // given
        let mut font = test_font();
        font.info.retain(|c| c.id != *b"ifil");

        // when
        let issues = validate(&font);

        // then
        assert!(has_errors(&issues));
    }

    #[test]
    fn validate_should_report_warning_when_bank_programs_duplicate() {
        // given
        let mut font = test_font();
        let mut duplicate = font.presets[0].clone();
        duplicate.name = crate::model::FixedName::from_text("dup");
        font.presets.push(duplicate);

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("duplicate bank:program"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_loop_points_outside_sample() {
        // given: end_loop one past the sample end, sample data untouched
        let mut font = test_font();
        font.samples[0].end_loop = font.samples[0].end + 1;

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("loop points")));
    }

    #[test]
    fn validate_should_report_warning_when_linked_sample_does_not_link_back() {
        // given
        let mut font = test_font();
        // samples 1 and 2 are the stereo pair in the fixture
        font.samples[2].sample_type = 1;
        font.samples[2].sample_link = 0;

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("not mutual")));
    }

    #[test]
    fn validate_should_report_warning_when_sample_link_is_dangling() {
        // given
        let mut font = test_font();
        font.samples[1].sample_link = 999;

        // when
        let issues = validate(&font);

        // then: broken links are tolerated (extraction sanitises them)
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("not mutual")));
    }

    #[test]
    fn issue_should_render_severity_prefix_when_displayed() {
        // given
        let issue = Issue {
            severity: Severity::Warning,
            message: "something odd".into(),
        };

        // when / then
        assert_eq!(format!("{issue}"), "warning: something odd");
    }

    #[test]
    fn validate_should_report_error_when_instrument_references_missing_sample() {
        // given
        let mut font = test_font();
        font.instruments[0].zones[0].gens[0].amount = 99;

        // when
        let issues = validate(&font);

        // then
        assert!(has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("references sample 99"))
        );
    }

    #[test]
    fn validate_should_accept_boundary_values_when_exactly_at_limits() {
        // given: bank 128, program 127, 48-point sample ending at the data end
        let mut font = test_font();
        font.presets[0].bank = 128;
        font.presets[0].program = 127;
        let start = font.samples[0].start;
        font.samples[0].end = start + 48;
        font.samples[0].start_loop = start;
        font.samples[0].end_loop = start + 48;
        let total = u32::try_from(font.sample_points()).unwrap();
        let kick_start = font.samples[3].start;
        font.samples[3].end = total;
        font.samples[3].start_loop = kick_start;
        font.samples[3].end_loop = total;

        // when
        let issues = validate(&font);

        // then: all values sit exactly on their limits, none may be flagged
        assert_eq!(issues, vec![]);
    }

    #[test]
    fn validate_should_not_error_when_sample_is_empty() {
        // given: start == end (zero-length sample)
        let mut font = test_font();
        let end = font.samples[0].end;
        font.samples[0].start = end;
        font.samples[0].start_loop = end;
        font.samples[0].end_loop = end;

        // when
        let issues = validate(&font);

        // then: warned about its length, but not an error
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("only 0 sample points"))
        );
    }

    #[test]
    fn validate_should_report_error_when_sample_start_exceeds_end() {
        // given
        let mut font = test_font();
        font.samples[0].start = font.samples[0].end + 1;

        // when
        let issues = validate(&font);

        // then
        assert!(has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("is after end point"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_bank_exceeds_128() {
        // given
        let mut font = test_font();
        font.presets[0].bank = 129;

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("bank 129 exceeds 128"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_program_exceeds_127() {
        // given
        let mut font = test_font();
        font.presets[0].program = 128;

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("program 128 exceeds 127"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_sample_is_shorter_than_48_points() {
        // given: a 47-point sample with consistent loop points
        let mut font = test_font();
        let start = font.samples[0].start;
        font.samples[0].end = start + 47;
        font.samples[0].start_loop = start;
        font.samples[0].end_loop = start + 47;

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("only 47 sample points"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_non_global_zone_lacks_reference() {
        // given: a second zone with no instrument generator
        let mut font = test_font();
        font.presets[0].zones = vec![Zone::default(), Zone::default()];

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("has no instrument reference"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_reference_is_not_final_generator() {
        // given
        let mut font = test_font();
        font.presets[0].zones[0].gens = vec![
            Generator {
                oper: GEN_INSTRUMENT,
                amount: 0,
            },
            Generator { oper: 8, amount: 1 },
        ];

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("not the final generator"))
        );
    }

    #[test]
    fn validate_should_report_warning_when_zone_has_duplicate_references() {
        // given
        let mut font = test_font();
        font.presets[0].zones[0].gens = vec![
            Generator {
                oper: GEN_INSTRUMENT,
                amount: 0,
            },
            Generator {
                oper: GEN_INSTRUMENT,
                amount: 1,
            },
        ];

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| {
            i.message
                .contains("2 instrument references; only the last is used")
        }));
    }

    #[test]
    fn validate_should_report_warning_when_version_major_is_not_2() {
        // given
        let mut font = test_font();
        font.info[0].data = vec![3, 0, 0, 0];

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("major version 3")));
    }

    #[test]
    fn validate_should_report_warning_when_recommended_info_chunks_missing() {
        // given
        let mut font = test_font();
        font.info.retain(|c| c.id == *b"ifil");

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("INAM")));
        assert!(issues.iter().any(|i| i.message.contains("isng")));
    }

    #[test]
    fn validate_should_report_warning_when_font_has_no_presets() {
        // given
        let mut font = test_font();
        font.presets.clear();

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("no presets")));
    }

    #[test]
    fn validate_should_report_warning_when_sm24_is_unusable() {
        // given
        let mut font = test_font();
        font.sample_data_24 = Some(vec![0; 3]);

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
        assert!(issues.iter().any(|i| i.message.contains("sm24")));
    }

    #[test]
    fn validate_should_skip_range_checks_when_sample_is_rom() {
        // given
        let mut font = test_font();
        font.samples[0].sample_type = crate::model::SAMPLE_TYPE_ROM | 1;
        font.samples[0].end = u32::MAX;

        // when
        let issues = validate(&font);

        // then
        assert!(!has_errors(&issues));
    }
}
