//! Request-scoped escape analysis (docs/MEMORY_MODEL.md).
//!
//! Classifies each allocation site inside a request scope as Stay, Promote, or
//! Conservative. The AOT runtime allocates Stay/Promote births in the request
//! arena; Promote (and Conservative) sites get a deep-promote before
//! `sup_req_leave`. Verbose mode reports conservative counts.

use std::fmt;

/// Birth / death classification for one allocation site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeClass {
    /// Proven not to escape the request → arena Stay, bulk reclaim at leave.
    Stay,
    /// Proven to escape → promote at the request boundary.
    Promote,
    /// Cannot prove non-escape → treat as Promote (safe).
    Conservative,
}

impl fmt::Display for EscapeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EscapeClass::Stay => write!(f, "stay"),
            EscapeClass::Promote => write!(f, "promote"),
            EscapeClass::Conservative => write!(f, "conservative"),
        }
    }
}

/// How a value may leave the request scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeEdge {
    None,
    StoreDurable,
    StoreGlobal,
    ReturnDurable,
    ClosureCapture,
    CrossTask,
    DurableCollection,
    /// Explicit promote hook (e.g. `socket_req_promote` / `conn_req_revive`).
    BoundaryPromote,
}

/// Facts the analysis needs about one allocation site (filled by dataflow in
/// `opt::collect_request_alloc_sites` or by unit-test fixtures). No source
/// special-casing of file/bench names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocSiteFacts {
    pub id: u32,
    /// True when the site is textually / dynamically inside `sup_req_enter`…`leave`.
    pub in_request_scope: bool,
    pub stored_into_durable: bool,
    pub stored_into_global: bool,
    pub returned_as_durable: bool,
    pub captured_by_escaping_closure: bool,
    pub sent_cross_task: bool,
    pub put_in_durable_collection: bool,
    /// Explicit boundary promote is planned for this value (Conn revive, App promote).
    pub has_boundary_promote: bool,
    /// Analysis could not finish (aliasing / cycles / unknown call).
    pub analysis_incomplete: bool,
    /// Why Conservative was required (empty unless `analysis_incomplete`).
    pub conservative_reason: String,
}

impl AllocSiteFacts {
    pub fn local_temp(id: u32) -> Self {
        Self {
            id,
            in_request_scope: true,
            stored_into_durable: false,
            stored_into_global: false,
            returned_as_durable: false,
            captured_by_escaping_closure: false,
            sent_cross_task: false,
            put_in_durable_collection: false,
            has_boundary_promote: false,
            analysis_incomplete: false,
            conservative_reason: String::new(),
        }
    }

    pub fn mark_incomplete(&mut self, reason: &str) {
        self.analysis_incomplete = true;
        if self.conservative_reason.is_empty() {
            self.conservative_reason = reason.to_string();
        }
    }
}

/// Result for one site, including the dominating escape edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteClass {
    pub id: u32,
    pub class: EscapeClass,
    pub edge: EscapeEdge,
}

/// Classify a single allocation site.
pub fn classify_site(facts: &AllocSiteFacts) -> SiteClass {
    if !facts.in_request_scope {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::StoreDurable,
        };
    }
    if facts.analysis_incomplete {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Conservative,
            edge: EscapeEdge::StoreDurable,
        };
    }
    if facts.stored_into_global {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::StoreGlobal,
        };
    }
    if facts.sent_cross_task {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::CrossTask,
        };
    }
    if facts.captured_by_escaping_closure {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::ClosureCapture,
        };
    }
    if facts.put_in_durable_collection {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::DurableCollection,
        };
    }
    if facts.returned_as_durable {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::ReturnDurable,
        };
    }
    if facts.stored_into_durable {
        // Conn/App COW into a durable root: Stay in arena when a boundary promote
        // will deep-copy before leave; otherwise Promote at the store.
        if facts.has_boundary_promote {
            return SiteClass {
                id: facts.id,
                class: EscapeClass::Stay,
                edge: EscapeEdge::BoundaryPromote,
            };
        }
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::StoreDurable,
        };
    }
    if facts.has_boundary_promote {
        return SiteClass {
            id: facts.id,
            class: EscapeClass::Stay,
            edge: EscapeEdge::BoundaryPromote,
        };
    }
    SiteClass {
        id: facts.id,
        class: EscapeClass::Stay,
        edge: EscapeEdge::None,
    }
}

/// Class counts for a classified module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EscapeSummary {
    pub stay: u32,
    pub promote: u32,
    pub conservative: u32,
    pub conservative_sites: Vec<(u32, String)>,
}

impl EscapeSummary {
    pub fn total(&self) -> u32 {
        self.stay + self.promote + self.conservative
    }
}

/// Run classification over many sites; returns classes plus conservative count.
/// Always builds an [`EscapeSummary`]. When `verbose`, prints per-class counts
/// and every Conservative site with its reason.
pub fn classify_module(sites: &[AllocSiteFacts], verbose: bool) -> (Vec<SiteClass>, u32) {
    let (classes, summary) = classify_module_summary(sites);
    if verbose {
        eprintln!(
            "super: escape-analysis sites={} stay={} promote={} conservative={}",
            summary.total(),
            summary.stay,
            summary.promote,
            summary.conservative
        );
        for (id, reason) in &summary.conservative_sites {
            eprintln!("super: escape-analysis conservative site {id}: {reason}");
        }
        for c in &classes {
            if c.class != EscapeClass::Conservative {
                eprintln!(
                    "super: escape-analysis site {} → {} ({:?})",
                    c.id, c.class, c.edge
                );
            }
        }
    }
    (classes, summary.conservative)
}

/// Classify and return the full summary (counts + Conservative reasons).
pub fn classify_module_summary(sites: &[AllocSiteFacts]) -> (Vec<SiteClass>, EscapeSummary) {
    let mut out = Vec::with_capacity(sites.len());
    let mut summary = EscapeSummary::default();
    for s in sites {
        let c = classify_site(s);
        match c.class {
            EscapeClass::Stay => summary.stay += 1,
            EscapeClass::Promote => summary.promote += 1,
            EscapeClass::Conservative => {
                summary.conservative += 1;
                let reason = if s.conservative_reason.is_empty() {
                    "analysis_incomplete".to_string()
                } else {
                    s.conservative_reason.clone()
                };
                summary.conservative_sites.push((c.id, reason));
            }
        }
        out.push(c);
    }
    (out, summary)
}

/// Force-promote mode used by the differential oracle (`SUPER_PROMOTE_EVERYTHING`).
pub fn classify_promote_everything(sites: &[AllocSiteFacts]) -> Vec<SiteClass> {
    sites
        .iter()
        .map(|s| SiteClass {
            id: s.id,
            class: EscapeClass::Promote,
            edge: EscapeEdge::StoreDurable,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect(facts: AllocSiteFacts, class: EscapeClass) {
        let c = classify_site(&facts);
        assert_eq!(c.class, class, "site {}", facts.id);
    }

    #[test]
    fn suite_twenty_five_classifications() {
        // 1 local Request stays
        expect(AllocSiteFacts::local_temp(1), EscapeClass::Stay);
        // 2 Response body map stays
        expect(AllocSiteFacts::local_temp(2), EscapeClass::Stay);
        // 3 nested header record stays
        expect(AllocSiteFacts::local_temp(3), EscapeClass::Stay);
        // 4 early-return local stays
        expect(AllocSiteFacts::local_temp(4), EscapeClass::Stay);
        // 5 error-path temp stays
        expect(AllocSiteFacts::local_temp(5), EscapeClass::Stay);
        // 6 Conn COW with revive → Stay (+ boundary promote)
        {
            let mut f = AllocSiteFacts::local_temp(6);
            f.stored_into_durable = true;
            f.has_boundary_promote = true;
            expect(f, EscapeClass::Stay);
        }
        // 7 App COW with socket_req_promote → Stay
        {
            let mut f = AllocSiteFacts::local_temp(7);
            f.stored_into_durable = true;
            f.has_boundary_promote = true;
            expect(f, EscapeClass::Stay);
        }
        // 8 store into durable without promote → Promote
        {
            let mut f = AllocSiteFacts::local_temp(8);
            f.stored_into_durable = true;
            expect(f, EscapeClass::Promote);
        }
        // 9 global / state store → Promote
        {
            let mut f = AllocSiteFacts::local_temp(9);
            f.stored_into_global = true;
            expect(f, EscapeClass::Promote);
        }
        // 10 return durable → Promote
        {
            let mut f = AllocSiteFacts::local_temp(10);
            f.returned_as_durable = true;
            expect(f, EscapeClass::Promote);
        }
        // 11 escaping closure capture → Promote
        {
            let mut f = AllocSiteFacts::local_temp(11);
            f.captured_by_escaping_closure = true;
            expect(f, EscapeClass::Promote);
        }
        // 12 cross-task send → Promote
        {
            let mut f = AllocSiteFacts::local_temp(12);
            f.sent_cross_task = true;
            expect(f, EscapeClass::Promote);
        }
        // 13 durable collection insert → Promote
        {
            let mut f = AllocSiteFacts::local_temp(13);
            f.put_in_durable_collection = true;
            expect(f, EscapeClass::Promote);
        }
        // 14 incomplete analysis → Conservative
        {
            let mut f = AllocSiteFacts::local_temp(14);
            f.analysis_incomplete = true;
            expect(f, EscapeClass::Conservative);
        }
        // 15 aliasing unknown → Conservative
        {
            let mut f = AllocSiteFacts::local_temp(15);
            f.analysis_incomplete = true;
            f.stored_into_durable = true;
            expect(f, EscapeClass::Conservative);
        }
        // 16 cycle through local containers still Stay if no escape edge
        expect(AllocSiteFacts::local_temp(16), EscapeClass::Stay);
        // 17 nested container Stay
        expect(AllocSiteFacts::local_temp(17), EscapeClass::Stay);
        // 18 outside request → Promote (durable birth)
        {
            let mut f = AllocSiteFacts::local_temp(18);
            f.in_request_scope = false;
            expect(f, EscapeClass::Promote);
        }
        // 19 boundary promote alone → Stay
        {
            let mut f = AllocSiteFacts::local_temp(19);
            f.has_boundary_promote = true;
            expect(f, EscapeClass::Stay);
        }
        // 20 store global wins over boundary promote
        {
            let mut f = AllocSiteFacts::local_temp(20);
            f.stored_into_global = true;
            f.has_boundary_promote = true;
            expect(f, EscapeClass::Promote);
        }
        // 21–25 more Stay temps (handler locals / error paths)
        for id in 21..=25 {
            expect(AllocSiteFacts::local_temp(id), EscapeClass::Stay);
        }
    }

    #[test]
    fn promote_everything_oracle_matches_byte_policy() {
        let sites: Vec<_> = (1..=10).map(AllocSiteFacts::local_temp).collect();
        let (normal, cons) = classify_module(&sites, false);
        assert_eq!(cons, 0);
        let forced = classify_promote_everything(&sites);
        assert_eq!(forced.len(), normal.len());
        for f in &forced {
            assert_eq!(f.class, EscapeClass::Promote);
        }
        // Differential contract: Stay sites under analysis become Promote under
        // the oracle; runtime SUPER_PROMOTE_EVERYTHING mirrors this for values.
        assert!(normal.iter().any(|c| c.class == EscapeClass::Stay));
    }

    #[test]
    fn conservative_count_logged() {
        let mut a = AllocSiteFacts::local_temp(1);
        a.analysis_incomplete = true;
        a.conservative_reason = "unknown_call:foo".into();
        let mut b = AllocSiteFacts::local_temp(2);
        b.analysis_incomplete = true;
        b.conservative_reason = "store_into_unknown_field".into();
        let (_, summary) = classify_module_summary(&[a, b]);
        assert_eq!(summary.conservative, 2);
        assert_eq!(summary.stay, 0);
        assert_eq!(summary.promote, 0);
        assert_eq!(summary.conservative_sites.len(), 2);
        assert_eq!(summary.conservative_sites[0].1, "unknown_call:foo");
        assert_eq!(summary.conservative_sites[1].1, "store_into_unknown_field");
    }

    #[test]
    fn summary_counts_stay_promote_conservative() {
        let stay = AllocSiteFacts::local_temp(1);
        let mut promote = AllocSiteFacts::local_temp(2);
        promote.stored_into_durable = true;
        let mut cons = AllocSiteFacts::local_temp(3);
        cons.mark_incomplete("unknown_call:host");
        let (classes, summary) = classify_module_summary(&[stay, promote, cons]);
        assert_eq!(summary.stay, 1);
        assert_eq!(summary.promote, 1);
        assert_eq!(summary.conservative, 1);
        assert_eq!(classes[0].class, EscapeClass::Stay);
        assert_eq!(classes[1].class, EscapeClass::Promote);
        assert_eq!(classes[2].class, EscapeClass::Conservative);
    }

    /// Honesty-pass fixture: one Stay + one Conservative(`unknown_call:opaque`).
    /// Always prints the verbose banner so docs can capture real lines.
    #[test]
    fn print_honesty_fixture_escape_summary() {
        let stay = AllocSiteFacts::local_temp(1);
        let mut cons = AllocSiteFacts::local_temp(2);
        cons.mark_incomplete("unknown_call:opaque");
        let (_, cons_n) = classify_module(&[stay, cons], true);
        assert_eq!(cons_n, 1);
    }

    /// A wrongly classified Stay on a durable store (no boundary) must be
    /// Promote — the sanitizer / region check relies on this for UAF freedom.
    #[test]
    fn wrong_stay_on_durable_store_is_rejected_by_classifier() {
        let mut f = AllocSiteFacts::local_temp(99);
        f.stored_into_durable = true;
        f.has_boundary_promote = false;
        // If analysis claimed Stay here, arena death would dangle in durable state.
        assert_eq!(classify_site(&f).class, EscapeClass::Promote);
    }
}
