/// Derive the risk_class from a set of risk_flags.
///
/// Priority order (highest wins): authority > security > architecture > low > normal.
/// The derivation is authoritative for the substrate; the gatekeeper agent does not
/// emit risk_class directly — it emits risk_flags, and the substrate derives risk_class.
///
/// From docs/risk-and-cluster-taxonomy.md § 3.
pub fn derive_risk_class(risk_flags: &[&str]) -> &'static str {
    if risk_flags.contains(&"touches_actor_authority")
        || risk_flags.contains(&"authority_surface_drift")
    {
        return "authority";
    }
    if risk_flags.contains(&"security_sensitive") {
        return "security";
    }
    if risk_flags.iter().any(|f| f.starts_with("touches_"))
        || risk_flags.contains(&"introduces_new_primitive")
        || risk_flags.contains(&"changes_boundary")
        || risk_flags.contains(&"contradicts_prior_decision")
    {
        return "architecture";
    }
    let low_flags: &[&str] = &["docs_only", "small_local_fix", "duplicate_symptom"];
    // Require at least one explicit low-risk flag; empty set falls through to "normal"
    // per docs/risk-and-cluster-taxonomy.md worked example row 6.
    if !risk_flags.is_empty() && risk_flags.iter().all(|f| low_flags.contains(f)) {
        return "low";
    }
    "normal"
}

/// Derive the approval_policy from size_tier, risk_class, and gatekeeper_decision.
///
/// Per docs/risk-and-cluster-taxonomy.md § 3 legal-combinations matrix:
/// - fast_track decision → auto (regardless of risk_class or size_tier)
/// - architecture / security / authority risk_class → architecture
/// - T0 + low/normal → auto; T1 + low → auto; T2 + low† → auto
/// - everything else → human
pub fn derive_approval_policy(
    size_tier: &str,
    risk_class: &str,
    gatekeeper_decision: &str,
) -> &'static str {
    if gatekeeper_decision == "fast_track" {
        return "auto";
    }
    if matches!(risk_class, "architecture" | "security" | "authority") {
        return "architecture";
    }
    // Matrix: T0 auto for low+normal; T1+low and T2+low also auto per § 3 table
    match (size_tier, risk_class) {
        ("T0", "low") | ("T0", "normal") | ("T1", "low") | ("T2", "low") => "auto",
        _ => "human",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- derive_risk_class ----

    #[test]
    fn authority_flag_wins_over_security() {
        assert_eq!(
            derive_risk_class(&["security_sensitive", "touches_actor_authority"]),
            "authority"
        );
    }

    #[test]
    fn authority_surface_drift_maps_to_authority() {
        assert_eq!(derive_risk_class(&["authority_surface_drift"]), "authority");
    }

    #[test]
    fn security_sensitive_alone_maps_to_security() {
        assert_eq!(derive_risk_class(&["security_sensitive"]), "security");
    }

    #[test]
    fn touches_lifecycle_maps_to_architecture() {
        assert_eq!(derive_risk_class(&["touches_lifecycle"]), "architecture");
    }

    #[test]
    fn touches_schema_core_maps_to_architecture() {
        assert_eq!(derive_risk_class(&["touches_schema_core"]), "architecture");
    }

    #[test]
    fn introduces_new_primitive_maps_to_architecture() {
        assert_eq!(derive_risk_class(&["introduces_new_primitive"]), "architecture");
    }

    #[test]
    fn changes_boundary_maps_to_architecture() {
        assert_eq!(derive_risk_class(&["changes_boundary"]), "architecture");
    }

    #[test]
    fn contradicts_prior_decision_maps_to_architecture() {
        assert_eq!(derive_risk_class(&["contradicts_prior_decision"]), "architecture");
    }

    #[test]
    fn docs_only_maps_to_low() {
        assert_eq!(derive_risk_class(&["docs_only"]), "low");
    }

    #[test]
    fn small_local_fix_maps_to_low() {
        assert_eq!(derive_risk_class(&["small_local_fix"]), "low");
    }

    #[test]
    fn duplicate_symptom_maps_to_low() {
        assert_eq!(derive_risk_class(&["duplicate_symptom"]), "low");
    }

    #[test]
    fn empty_flags_maps_to_normal() {
        // No explicit risk flags → normal per docs worked example row 6 (ordinary substrate work).
        assert_eq!(derive_risk_class(&[]), "normal");
    }

    #[test]
    fn low_flags_combination_maps_to_low() {
        assert_eq!(
            derive_risk_class(&["docs_only", "small_local_fix", "duplicate_symptom"]),
            "low"
        );
    }

    // ---- derive_approval_policy ----

    // Table row 1: T0 + low → auto (per matrix cell)
    #[test]
    fn t0_low_maps_to_auto() {
        assert_eq!(derive_approval_policy("T0", "low", "normal_observation"), "auto");
    }

    // Table row 1: T0 + normal → auto (per matrix cell)
    #[test]
    fn t0_normal_maps_to_auto() {
        assert_eq!(derive_approval_policy("T0", "normal", "normal_observation"), "auto");
    }

    // Table row 1: T0 + architecture → architecture
    #[test]
    fn t0_architecture_maps_to_architecture() {
        assert_eq!(derive_approval_policy("T0", "architecture", "arch_review_candidate"), "architecture");
    }

    // Table row 2: T1 + low → auto (fast_track)
    #[test]
    fn t1_low_fast_track_maps_to_auto() {
        assert_eq!(derive_approval_policy("T1", "low", "fast_track"), "auto");
    }

    // Table row 2: T1 + low → auto (matrix cell — not just fast_track path)
    #[test]
    fn t1_low_normal_observation_maps_to_auto() {
        assert_eq!(derive_approval_policy("T1", "low", "normal_observation"), "auto");
    }

    // Table row 2: T1 + normal → human
    #[test]
    fn t1_normal_maps_to_human() {
        assert_eq!(derive_approval_policy("T1", "normal", "normal_observation"), "human");
    }

    // Table row 2: T1 + architecture → architecture
    #[test]
    fn t1_architecture_maps_to_architecture() {
        assert_eq!(derive_approval_policy("T1", "architecture", "arch_review_candidate"), "architecture");
    }

    // Table row 3: T2 + low → auto (fast_track)
    #[test]
    fn t2_low_fast_track_maps_to_auto() {
        assert_eq!(derive_approval_policy("T2", "low", "fast_track"), "auto");
    }

    // Table row 3: T2 + low → auto† (matrix cell — one-phase plan with purely cosmetic work)
    #[test]
    fn t2_low_normal_observation_maps_to_auto() {
        assert_eq!(derive_approval_policy("T2", "low", "normal_observation"), "auto");
    }

    // Table row 3: T2 + normal → human
    #[test]
    fn t2_normal_maps_to_human() {
        assert_eq!(derive_approval_policy("T2", "normal", "normal_observation"), "human");
    }

    // Table row 3: T2 + security → architecture
    #[test]
    fn t2_security_maps_to_architecture() {
        assert_eq!(derive_approval_policy("T2", "security", "arch_review_candidate"), "architecture");
    }

    // Table row 3: T2 + authority → architecture
    #[test]
    fn t2_authority_maps_to_architecture() {
        assert_eq!(derive_approval_policy("T2", "authority", "arch_review_candidate"), "architecture");
    }

    // Table row 4: T3 + normal → human
    #[test]
    fn t3_normal_maps_to_human() {
        assert_eq!(derive_approval_policy("T3", "normal", "normal_observation"), "human");
    }

    // Table row 4: T3 + architecture → architecture
    #[test]
    fn t3_architecture_maps_to_architecture() {
        assert_eq!(derive_approval_policy("T3", "architecture", "arch_review_candidate"), "architecture");
    }

    // fast_track always → auto regardless of risk_class
    #[test]
    fn fast_track_always_auto() {
        for risk in &["low", "normal", "architecture"] {
            assert_eq!(
                derive_approval_policy("T1", risk, "fast_track"),
                "auto",
                "fast_track + {risk} must be auto"
            );
        }
    }
}
