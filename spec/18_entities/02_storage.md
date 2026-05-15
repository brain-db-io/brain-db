# Entity Storage Layout

## redb tables

### `entities`
```
key:   EntityId (16 bytes)
value: rkyv-serialized Entity (canonical_name, aliases, type, attributes, mention_count, timestamps, merged_into, embedding_version)
```

### `entity_by_canonical_name`
```
key:   (entity_type_id: u32, normalized_name: String)
value: EntityId
```
Secondary index for tier-1 exact resolution.

### `entity_aliases`
```
key:   (entity_type_id: u32, normalized_alias: String, entity_id: EntityId)
value: () (membership only)
```
Aliases index for tier-1 resolution. Composite key allows the same alias to map to entities of different types.

### `entity_trigrams`
```
key:   (entity_type_id: u32, trigram: [u8; 3], entity_id: EntityId)
value: ()
```
Trigram index for tier-2 fuzzy resolution. Each entity's canonical_name contributes its trigrams. Similarity scored at query time via index intersection + Jaccard.

### `entity_mentions`
```
key:   (entity_id: EntityId, memory_id: MemoryId)
value: MentionMetadata (offset in memory text, confidence, extractor_id)
```
Reverse: which memories mention each entity. Used for graph queries and provenance.

### `entity_resolution_audit`
```
key:   AuditId (16 bytes)
value: rkyv-serialized ResolutionAudit
```

### `entity_merge_log`
```
key:   (timestamp: u64, merge_id: u128)
value: MergeRecord { survivor, merged, confidence, actor, attributes_resolution }
```

## Entity embedding HNSW

A per-shard HNSW index, separate from the main memory HNSW:

- Index: `entity_embeddings.hnsw`
- Vector dim: 384 (same as memory)
- Parameters: M=16, ef_construction=100 (lower than memory; entity count is smaller), ef_search=64
- Tombstoned entities are removed via the standard HNSW tombstone+rebuild cycle.

## Storage costs

For a deployment with N entities, each averaging:
- canonical_name: 30 chars
- aliases: 5 × 30 chars = 150 chars
- 5 attributes averaging 50 bytes each = 250 bytes
- embedding: 1536 bytes

Per entity: ~2 KB in the main table.
Plus index entries: ~200 bytes per entity across all indexes.
Plus HNSW: ~3 KB per entity (vector + HNSW links).

Total: ~5 KB per entity. 100K entities = 500 MB. 1M entities = 5 GB.

This is small relative to memory storage (memories are typically 2 KB of text + 1.6 KB slot = ~4 KB each, with M memories typically >> N entities).

## Read paths

| Query | Path |
|---|---|
| Get entity by ID | redb `entities` lookup (O(log N) seek) |
| Exact name resolution | `entity_by_canonical_name` or `entity_aliases` lookup |
| Fuzzy resolution | `entity_trigrams` intersection of candidate trigrams, scored |
| Embedding resolution | Entity HNSW search |
| All memories mentioning entity | `entity_mentions` prefix scan |
| All statements with subject = entity | `statements_by_subject` index (see `19_statements/`) |
| All relations involving entity | `relations_by_from` + `relations_by_to` (see `20_relations/`) |

## Write paths

Entity creation (new):
1. Generate EntityId (UUIDv7).
2. Write to `entities`.
3. Write to `entity_by_canonical_name`.
4. Write trigrams to `entity_trigrams`.
5. Embed and write to entity HNSW (async, doesn't block).
6. Commit redb transaction.

All steps except 5 are in a single redb transaction (single-writer-per-shard discipline from the substrate). Step 5 is async; embedding-based resolution may miss this entity for a few seconds.

Entity update (rename, attribute change):
1. Read current entity.
2. Compute delta (which indexes need update).
3. Update `entities`.
4. Update `entity_by_canonical_name` (remove old, add new) if canonical_name changed.
5. Update `entity_trigrams` (remove old set, add new) if canonical_name changed.
6. Queue async re-embedding.

Entity merge: see `01_resolution.md` for the full procedure.
