//! Compact identifier formatting.
//!
//! Memory ids are 128-bit triples; entity / statement ids are 16-byte
//! UUIDs. The full forms dominate a terminal line; the table renderers
//! want a short canonical form humans can copy out of `psql`-style
//! output and feed back as a query. These helpers centralize that
//! convention so the shell, the CLI, and (later) the TUI all agree.

use brain_core::MemoryId;

/// Compact `s{shard}/m{slot}/v{version}` form of a [`MemoryId`].
///
/// Inverse of `brain-shell`'s `parse_short_form` — the canonical short
/// form a user types back to refer to a recalled memory.
///
/// ```
/// use brain_core::MemoryId;
/// use brain_explore::util::short_id::memory_id_short_form;
///
/// let id = MemoryId::pack(7, 42, 3);
/// assert_eq!(memory_id_short_form(id), "s7/m42/v3");
/// ```
#[must_use]
pub fn memory_id_short_form(id: MemoryId) -> String {
    format!("s{}/m{}/v{}", id.shard(), id.slot(), id.version())
}

/// First 4 bytes (8 hex chars) + `…` form of a 16-byte UUID-shaped id.
///
/// Used for entity ids, statement ids, agent ids, request ids, and any
/// other UUID-shaped identifier whose full form is too long for a table
/// cell. Eight hex chars give 32 bits of disambiguation — enough that
/// collisions in a recalled batch are effectively impossible.
#[must_use]
pub fn uuid_short_form(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}…",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

/// Short form of a 16-byte entity id. Today identical to
/// [`uuid_short_form`]; kept as its own function so the EntityId
/// representation can evolve (e.g. a typed wrapper) without rippling.
#[must_use]
pub fn entity_id_short_form(bytes: &[u8; 16]) -> String {
    uuid_short_form(bytes)
}

/// Short form of a 16-byte statement id. See [`entity_id_short_form`].
#[must_use]
pub fn statement_id_short_form(bytes: &[u8; 16]) -> String {
    uuid_short_form(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_id_short_form_roundtrips() {
        // The short form is the canonical user-facing identifier; it
        // must round-trip cleanly through the pack/unpack accessors so
        // the shell's parser can reconstruct the id from the rendered
        // string.
        let id = MemoryId::pack(7, 42, 3);
        assert_eq!(memory_id_short_form(id), "s7/m42/v3");

        let id = MemoryId::pack(0, 0, 0);
        assert_eq!(memory_id_short_form(id), "s0/m0/v0");

        // u16::MAX shard, large slot, version near u32::MAX.
        let id = MemoryId::pack(u16::MAX, 0x1234_5678, 99_999);
        assert_eq!(memory_id_short_form(id), "s65535/m305419896/v99999");
    }

    #[test]
    fn uuid_short_form_uses_first_four_bytes() {
        let bytes = [
            0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33,
            0x44, 0x55,
        ];
        assert_eq!(uuid_short_form(&bytes), "abcdef12…");
    }

    #[test]
    fn uuid_short_form_zero_bytes() {
        let bytes = [0u8; 16];
        assert_eq!(uuid_short_form(&bytes), "00000000…");
    }
}
