//! Host-recorded context exposure at the time an event is captured.
//!
//! These flags concern recorded exposure, not human authorship, semantic truth
//! or statistical independence. Generated text remains ineligible regardless
//! of these flags. Original user-channel text can remain an inspectable
//! observation after exposure, but it cannot count as unexposed support.

use serde::{Deserialize, Serialize};

/// Bind this policy version into episode revisions when projecting evidence.
pub(super) const VERSION: &str = "hook-event-influence-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Influence {
    pub prior_profile_context: bool,
    pub prior_refiner_context: bool,
    pub prior_unknown: bool,
}

impl Default for Influence {
    fn default() -> Self {
        // An absent field in a legacy capture is missing provenance. It must
        // never become an affirmative assertion that no context was offered.
        Self {
            prior_profile_context: false,
            prior_refiner_context: false,
            prior_unknown: true,
        }
    }
}

impl Influence {
    /// Only a newly observed capture session may start with this state. A
    /// resumed or legacy session must preserve its state or remain unknown.
    pub(super) fn fresh() -> Self {
        Self {
            prior_profile_context: false,
            prior_refiner_context: false,
            prior_unknown: false,
        }
    }

    /// Call after recording the prompt and before publishing profile context.
    /// Mutate the session accumulator, never the captured event's snapshot.
    pub(super) fn offer_profile(&mut self) {
        self.prior_profile_context = true;
    }

    /// Even passthrough offers routing advice. Later input has prior refiner
    /// exposure without claiming that the client applied the offered advice.
    pub(super) fn offer_refiner(&mut self) {
        self.prior_refiner_context = true;
    }

    pub(super) fn class(self) -> EvidenceClass {
        if self.prior_unknown {
            EvidenceClass::UnknownInfluence
        } else if self.prior_profile_context || self.prior_refiner_context {
            EvidenceClass::DependentObservation
        } else {
            EvidenceClass::NoRecordedPriorExposure
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EvidenceClass {
    NoRecordedPriorExposure,
    DependentObservation,
    UnknownInfluence,
}

/// Aggregate sources after normal origin and quotation validation. The caller
/// supplies every support and contradiction, not only favorable observations.
/// A model cannot choose the result. Empty input remains unknown.
pub(super) fn classify_sources<'a>(
    sources: impl IntoIterator<Item = &'a Influence>,
) -> EvidenceClass {
    let mut found = false;
    let mut combined = Influence::fresh();
    for source in sources {
        found = true;
        combined.prior_profile_context |= source.prior_profile_context;
        combined.prior_refiner_context |= source.prior_refiner_context;
        combined.prior_unknown |= source.prior_unknown;
    }
    if found {
        combined.class()
    } else {
        EvidenceClass::UnknownInfluence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_offers_do_not_reclassify_the_original_prompt() {
        let mut session = Influence::fresh();
        let original = session;
        session.offer_refiner();
        let next_prompt = session;
        session.offer_profile();

        assert_eq!(original.class(), EvidenceClass::NoRecordedPriorExposure);
        assert_eq!(next_prompt.class(), EvidenceClass::DependentObservation);
        assert!(next_prompt.prior_refiner_context);
        assert!(!next_prompt.prior_profile_context);
        assert!(session.prior_refiner_context && session.prior_profile_context);
        assert_eq!(session.class(), EvidenceClass::DependentObservation);
    }

    #[test]
    fn prior_profile_exposure_survives_current_refinement_and_repeated_offers() {
        let mut session = Influence::fresh();
        session.offer_profile();
        let original = session;
        for _ in 0..64 {
            session.offer_refiner();
            session.offer_profile();
        }
        assert_eq!(original.class(), EvidenceClass::DependentObservation);
        assert!(original.prior_profile_context);
        assert!(!original.prior_refiner_context);
        assert_eq!(
            session,
            Influence {
                prior_profile_context: true,
                prior_refiner_context: true,
                prior_unknown: false,
            }
        );
    }

    #[test]
    fn missing_provenance_remains_unknown_after_new_observations() {
        let mut legacy = Influence::default();
        legacy.offer_refiner();
        legacy.offer_profile();
        assert_eq!(legacy.class(), EvidenceClass::UnknownInfluence);
        assert!(legacy.prior_refiner_context && legacy.prior_profile_context);

        let encoded = serde_json::to_vec(&legacy).unwrap();
        assert_eq!(
            serde_json::from_slice::<Influence>(&encoded).unwrap(),
            legacy
        );
        assert!(serde_json::from_str::<Influence>(
            r#"{"prior_profile_context":false,"prior_refiner_context":false}"#,
        )
        .is_err());
        assert!(serde_json::from_str::<Influence>(
            r#"{"prior_profile_context":false,"prior_refiner_context":false,"prior_unknown":false,"human":true}"#,
        )
        .is_err());
    }

    #[test]
    fn every_cited_source_contributes_to_the_host_classification() {
        let unexposed = Influence::fresh();
        let mut dependent = Influence::fresh();
        dependent.offer_refiner();
        let unknown = Influence::default();
        assert_eq!(
            classify_sources([&unexposed, &unexposed]),
            EvidenceClass::NoRecordedPriorExposure
        );
        assert_eq!(
            classify_sources([&unexposed, &dependent]),
            EvidenceClass::DependentObservation
        );
        assert_eq!(
            classify_sources([&dependent, &unknown, &unexposed]),
            EvidenceClass::UnknownInfluence
        );
        assert_eq!(
            classify_sources(std::iter::empty()),
            EvidenceClass::UnknownInfluence
        );
    }
}
