//! Commit-vote statistics shared by the style and workflow detectors. The unit
//! of evidence is the commit: lines within one commit share one decision and
//! often one formatter run, so each commit with an opportunity votes once.

use super::store::Counts;

/// Commits that must have had the opportunity before a claim is considered.
/// These are descriptive Wilson-score tiers with z=1.96, not calibrated
/// probabilities of a personal trait. Git commits are not iid Bernoulli trials.
pub(super) const MIN_COMMITS: usize = 8;
const MEDIUM_AGREEMENT: f64 = 0.6;
const HIGH_AGREEMENT: f64 = 0.8;
const WILSON_Z: f64 = 1.96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Confidence {
    High,
    Medium,
}

impl Confidence {
    pub(super) fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
        }
    }
}

pub(super) fn bump(c: &mut Counts, key: &str, n: i64) {
    *c.entry(key.to_string()).or_insert(0) += n;
}

pub(super) fn cget(c: &Counts, key: &str) -> i64 {
    c.get(key).copied().unwrap_or(0)
}

/// Commits that had the opportunity to show a pattern, and how many did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Support {
    pub(super) agree: usize,
    pub(super) commits: usize,
}

impl Support {
    pub(super) fn label(self) -> String {
        format!("{}/{} commits", self.agree, self.commits)
    }
}

/// Each eligible commit votes once: true supports the predicate, false does
/// not support it (including an ambiguous tie), None was not measurable.
pub(super) fn support(commits: &[&Counts], vote: impl Fn(&Counts) -> Option<bool>) -> Support {
    let mut support = Support {
        agree: 0,
        commits: 0,
    };
    for counts in commits {
        if let Some(agrees) = vote(counts) {
            support.commits += 1;
            support.agree += usize::from(agrees);
        }
    }
    support
}

/// One commit's majority between two tallies, compared with the claimed side.
pub(super) fn majority(c: &Counts, yes: &str, no: &str, claim_yes: bool) -> Option<bool> {
    let (yes, no) = (cget(c, yes), cget(c, no));
    (yes > 0 || no > 0).then_some(if claim_yes { yes > no } else { no > yes })
}

/// Choose the predicate by the same commit votes used by the score. Replicating
/// lines within a commit without changing its majority cannot change the side.
pub(super) fn dominant(commits: &[&Counts], yes: &str, no: &str) -> (bool, Support) {
    let yes_support = support(commits, |c| majority(c, yes, no, true));
    let no_support = support(commits, |c| majority(c, yes, no, false));
    if yes_support.agree >= no_support.agree {
        (true, yes_support)
    } else {
        (false, no_support)
    }
}

/// A claim survives only when enough commits had the opportunity and the lower
/// Wilson bound of their agreement clears the tier. Returns `None` (→ no rule)
/// when the signal is weak.
pub(super) fn gate(support: Support) -> Option<Confidence> {
    if support.commits < MIN_COMMITS || support.agree > support.commits {
        return None;
    }
    let lower = wilson_lower_bound(support.agree, support.commits);
    if lower >= HIGH_AGREEMENT {
        Some(Confidence::High)
    } else if lower >= MEDIUM_AGREEMENT {
        Some(Confidence::Medium)
    } else {
        None
    }
}

fn wilson_lower_bound(agree: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let n = total as f64;
    let p = agree as f64 / n;
    let z2 = WILSON_Z * WILSON_Z;
    let centre = p + z2 / (2.0 * n);
    let margin = WILSON_Z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    (centre - margin) / (1.0 + z2 / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_counts_agreeing_commits_not_lines() {
        let support = |agree, commits| Support { agree, commits };
        assert_eq!(gate(support(20, 20)), Some(Confidence::High));
        assert_eq!(gate(support(8, 8)), Some(Confidence::Medium));
        assert_eq!(gate(support(27, 30)), Some(Confidence::Medium));
        assert_eq!(gate(support(7, 7)), None); // too few commits had the opportunity
        assert_eq!(gate(support(12, 20)), None); // no clear agreement
        assert!((wilson_lower_bound(8, 8) - 0.676).abs() < 0.001);
        assert_eq!(wilson_lower_bound(0, 0), 0.0);
        assert_eq!(gate(support(21, 20)), None);
    }

    #[test]
    fn ties_are_eligible_but_do_not_support_either_strict_majority() {
        let yes = Counts::from([("yes".into(), 2), ("no".into(), 0)]);
        let tied = Counts::from([("yes".into(), 1), ("no".into(), 1)]);
        let mut commits = vec![&yes; 20];
        commits.extend(vec![&tied; 980]);
        let (_, vote) = dominant(&commits, "yes", "no");
        assert_eq!(
            vote,
            Support {
                agree: 20,
                commits: 1000
            }
        );
        assert_eq!(gate(vote), None);
        assert_eq!(majority(&tied, "yes", "no", true), Some(false));
        assert_eq!(majority(&tied, "yes", "no", false), Some(false));
        assert_eq!(majority(&Counts::new(), "yes", "no", true), None);
    }

    #[test]
    fn line_multiplication_cannot_reverse_commit_vote_direction() {
        let yes = Counts::from([("yes".into(), 3), ("no".into(), 1)]);
        for scale in [1, 100, 1000000] {
            let no = Counts::from([("yes".into(), scale), ("no".into(), scale * 3)]);
            let mut commits = vec![&yes; 20];
            commits.push(&no);
            assert_eq!(
                dominant(&commits, "yes", "no"),
                (
                    true,
                    Support {
                        agree: 20,
                        commits: 21
                    }
                )
            );
        }
    }

    #[test]
    fn wilson_matches_the_inverted_score_test_over_the_finite_domain() {
        // Independent algebraic oracle, rather than repeating the implementation
        // of the quadratic root. 4,006,000 threshold comparisons.
        for n in 1..=2000 {
            let mut previous = -1.0;
            for k in 0..=n {
                let lower = wilson_lower_bound(k, n);
                assert!(lower >= -1e-14 && lower <= k as f64 / n as f64 + 1e-14);
                assert!(lower >= previous - 1e-14);
                previous = lower;
                for q in [MEDIUM_AGREEMENT, HIGH_AGREEMENT] {
                    let threshold = n as f64 * q + WILSON_Z * (n as f64 * q * (1.0 - q)).sqrt();
                    assert_eq!(lower >= q, k as f64 >= threshold, "k={k}, n={n}, q={q}");
                }
            }
        }
    }
}
