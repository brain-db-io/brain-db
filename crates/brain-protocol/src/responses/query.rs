//! Hybrid-query response wire types.
//!
//! Request side: [`crate::requests::query`]. Shared wire-domain types
//! (`TimeRangeWire`, `RetrieverWire`, `RetrieverSelectionWire`,
//! `FusionConfigWire`, `ItemIdWire`, `RetrieverContributionWire`,
//! `RetrieverOutcomeWire`) are defined on the request side and
//! re-exported here so callers reading a response can pull them from
//! one place.

use rkyv::{Archive, Deserialize, Serialize};

// Re-export the shared types so callers can `use brain_protocol::responses::query::{...}`.
pub use crate::requests::query::{
    FusionConfigWire, ItemIdWire, RetrieverContributionWire, RetrieverOutcomeWire,
    RetrieverSelectionWire, RetrieverWire, TimeRangeWire,
};

// ---------------------------------------------------------------------------
// QUERY (0x0160) — response side.
// ---------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct QueryResultItem {
    pub id: ItemIdWire,
    pub fused_score: f64,
    pub contributing: Vec<RetrieverContributionWire>,
}

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct QueryResponse {
    pub items: Vec<QueryResultItem>,
    pub total_latency_ms: f64,
    pub retriever_outcomes: Vec<RetrieverOutcomeWire>,
}

// ---------------------------------------------------------------------------
// QUERY_EXPLAIN (0x0161) — response side.
// ---------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct QueryExplainResponse {
    pub plan_text: String,
    pub estimated_cost_ms: f32,
}

// ---------------------------------------------------------------------------
// QUERY_TRACE (0x0162) — response side.
// ---------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct QueryTraceResponse {
    pub trace_text: String,
    pub total_latency_ms: f64,
}

// ---------------------------------------------------------------------------
// RECALL_HYBRID (0x0163) — response side.
// ---------------------------------------------------------------------------

#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct MemoryHit {
    /// Big-endian bytes of the u128 MemoryId.
    pub memory_id: [u8; 16],
    pub fused_score: f64,
}

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct RecallHybridResponse {
    pub items: Vec<MemoryHit>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> T
    where
        T: rkyv::Archive + rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<256>> + Clone,
        T::Archived: rkyv::Deserialize<T, rkyv::Infallible>
            + for<'a> rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    {
        let bytes = rkyv::to_bytes::<_, 256>(value).expect("rkyv ser");
        let archived = rkyv::check_archived_root::<T>(&bytes).expect("check");
        archived.deserialize(&mut rkyv::Infallible).expect("deser")
    }

    #[test]
    fn query_response_round_trips() {
        let v = QueryResponse {
            items: vec![QueryResultItem {
                id: ItemIdWire {
                    kind: 0,
                    bytes: [1u8; 16],
                },
                fused_score: 0.0164,
                contributing: vec![RetrieverContributionWire {
                    retriever: RetrieverWire::Semantic,
                    rank: 1,
                    raw_score: 0.9,
                }],
            }],
            total_latency_ms: 12.3,
            retriever_outcomes: vec![RetrieverOutcomeWire {
                retriever: RetrieverWire::Semantic,
                status: 0,
                message: String::new(),
                latency_ms: 5.2,
                result_count: 1,
            }],
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn query_explain_round_trips() {
        let v = QueryExplainResponse {
            plan_text: "PLAN: ...".into(),
            estimated_cost_ms: 12.5,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn query_trace_round_trips() {
        let v = QueryTraceResponse {
            trace_text: "PLAN ... EXECUTION ...".into(),
            total_latency_ms: 22.4,
        };
        assert_eq!(round_trip(&v), v);
    }

    #[test]
    fn recall_hybrid_response_round_trips() {
        let v = RecallHybridResponse {
            items: vec![MemoryHit {
                memory_id: [3u8; 16],
                fused_score: 0.05,
            }],
        };
        assert_eq!(round_trip(&v), v);
    }
}
