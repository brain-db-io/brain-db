//! Procedural-memory materialization — response side (W3.1, wire v2).
//!
//! Request side: [`crate::requests::procedural`].

use rkyv::{Archive, Deserialize, Serialize};

use crate::request::WireUuid;

/// `MATERIALIZE_PROCEDURAL_RESP` (`0x01E4`).
///
/// `system_block` is fully rendered Markdown suitable for injection
/// into an LLM system prompt. `statement_ids` lists every contributing
/// statement (in rendering order) so callers can audit / explain.
/// `total_candidates` reports how many behavior_* statements matched
/// before the `top_k` cap; `trimmed_by_budget` is true iff the cap
/// actually fired (`total_candidates > top_k`).
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct MaterializeProceduralResponse {
    pub system_block: String,
    pub statement_ids: Vec<WireUuid>,
    pub total_candidates: u32,
    pub trimmed_by_budget: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcode::Opcode;
    use crate::response::ResponseBody;
    use crate::rkyv_codec::{from_rkyv_bytes, to_rkyv_bytes};

    fn sample_uuid(seed: u8) -> WireUuid {
        let mut u = [0u8; 16];
        for (i, b) in u.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        u
    }

    #[test]
    fn opcode_byte_assignment() {
        assert_eq!(Opcode::MaterializeProceduralResp.as_u16(), 0x01E4);
        assert!(Opcode::MaterializeProceduralResp.is_response());
    }

    #[test]
    fn response_round_trips_via_rkyv() {
        let resp = MaterializeProceduralResponse {
            system_block: "# Learned behaviors\n\n- be concise\n".into(),
            statement_ids: vec![sample_uuid(3), sample_uuid(4)],
            total_candidates: 5,
            trimmed_by_budget: true,
        };
        let bytes = to_rkyv_bytes(&resp);
        let back: MaterializeProceduralResponse = from_rkyv_bytes(&bytes).unwrap();
        assert_eq!(back, resp);
    }

    #[test]
    fn response_round_trips_through_response_body() {
        let resp = MaterializeProceduralResponse {
            system_block: String::new(),
            statement_ids: Vec::new(),
            total_candidates: 0,
            trimmed_by_budget: false,
        };
        let body = ResponseBody::MaterializeProcedural(resp.clone());
        assert_eq!(body.opcode(), Opcode::MaterializeProceduralResp);
        let bytes = body.encode();
        let decoded = ResponseBody::decode(Opcode::MaterializeProceduralResp, &bytes).unwrap();
        assert_eq!(decoded, ResponseBody::MaterializeProcedural(resp));
    }
}
