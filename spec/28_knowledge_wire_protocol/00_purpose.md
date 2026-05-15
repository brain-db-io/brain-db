# Wire Protocol — Knowledge Operations

The wire protocol's foundation (32-byte frame header, opcode + body, CRC32C, rkyv-serialized bodies) is defined in section 03. This section defines the opcodes that operate on the knowledge layer: schema, entities, statements, relations, queries, and admin.

The substrate's cognitive primitives (ENCODE, RECALL, PLAN, REASON, FORGET, LINK, UNLINK, TXN_*, SUBSCRIBE) are defined in section 03. Together with the opcodes here, they form the complete wire surface.

## Opcode space

```
0x00–0x0F   reserved
0x10–0x1F   cognitive primitives (defined in section 03)
0x20–0x2F   schema operations
0x30–0x3F   entity operations
0x40–0x4F   statement operations
0x50–0x5F   relation operations
0x60–0x6F   query operations (hybrid retrieval)
0x70–0x7F   admin operations
0x80–0x8F   reserved future
```

## Schema operations (0x20–0x2F)

| Opcode | Name | Body | Response |
|---|---|---|---|
| 0x20 | SCHEMA_UPLOAD | schema document (text) | schema_version, validation_errors |
| 0x21 | SCHEMA_GET | version_id (latest if 0) | schema document |
| 0x22 | SCHEMA_LIST | (none) | list of versions with timestamps |
| 0x23 | SCHEMA_VALIDATE | schema document | validation_errors (without commit) |
| 0x24 | EXTRACTOR_LIST | (none) | active extractors |
| 0x25 | EXTRACTOR_DISABLE | extractor_id | confirmation |
| 0x26 | EXTRACTOR_ENABLE | extractor_id | confirmation |

## Entity operations (0x30–0x3F)

| Opcode | Name | Body | Response |
|---|---|---|---|
| 0x30 | ENTITY_CREATE | type, canonical_name, attributes | EntityId |
| 0x31 | ENTITY_GET | EntityId | Entity record |
| 0x32 | ENTITY_UPDATE | EntityId, attribute_deltas | confirmation |
| 0x33 | ENTITY_RENAME | EntityId, new_name, move_to_alias | confirmation |
| 0x34 | ENTITY_MERGE | survivor, merged, confidence | merge audit_id |
| 0x35 | ENTITY_UNMERGE | merged_entity | restored EntityId |
| 0x36 | ENTITY_RESOLVE | candidate_name, context, hint | ResolutionOutcome |
| 0x37 | ENTITY_LIST | filter (type, name_prefix, mention_count_min) | EntityIds |
| 0x38 | ENTITY_TOMBSTONE | EntityId, reason | confirmation |

## Statement operations (0x40–0x4F)

| Opcode | Name | Body | Response |
|---|---|---|---|
| 0x40 | STATEMENT_CREATE | kind, subject, predicate, object, evidence, confidence | StatementId |
| 0x41 | STATEMENT_GET | StatementId | Statement record |
| 0x42 | STATEMENT_SUPERSEDE | old_id, new_statement | new StatementId |
| 0x43 | STATEMENT_TOMBSTONE | StatementId, reason | confirmation |
| 0x44 | STATEMENT_RETRACT | StatementId | confirmation |
| 0x45 | STATEMENT_HISTORY | StatementId or chain_root | full chain |
| 0x46 | STATEMENT_LIST | filter (subject, predicate, kind, time, confidence) | StatementIds |

## Relation operations (0x50–0x5F)

| Opcode | Name | Body | Response |
|---|---|---|---|
| 0x50 | RELATION_CREATE | type, from, to, properties, evidence | RelationId |
| 0x51 | RELATION_GET | RelationId | Relation record |
| 0x52 | RELATION_SUPERSEDE | old_id, new_relation | new RelationId |
| 0x53 | RELATION_TOMBSTONE | RelationId, reason | confirmation |
| 0x54 | RELATION_LIST_FROM | EntityId, type_filter, time_filter | RelationIds |
| 0x55 | RELATION_LIST_TO | EntityId, type_filter, time_filter | RelationIds |
| 0x56 | RELATION_TRAVERSE | start, types, depth, direction | path/subgraph |

## Query operations (0x60–0x6F)

| Opcode | Name | Body | Response |
|---|---|---|---|
| 0x60 | QUERY | QueryRequest | QueryResult (streamed if large) |
| 0x61 | QUERY_EXPLAIN | QueryRequest | QueryPlan (no execution) |
| 0x62 | QUERY_TRACE | QueryRequest | QueryResult + per-retriever debug |
| 0x63 | RECALL_HYBRID | text, filters, retriever_selection | RecallResult |

`QUERY` is the primary structured query opcode. `RECALL_HYBRID` is the simple-text fast path used by clients that just want hybrid text-only retrieval with no entity anchoring.

The simpler `RECALL` opcode (section 03, 0x11) is the substrate-level vector recall. When a schema is declared, the server routes `RECALL` through the hybrid retriever transparently — clients see the same response shape with additional metadata fields (`contributing_retrievers`, etc.).

## Admin operations (0x70–0x7F)

| Opcode | Name | Body | Response |
|---|---|---|---|
| 0x70 | ADMIN_REBUILD_INDEX | index_name, shard_id | job_id |
| 0x71 | ADMIN_REINDEX_TANTIVY | shard_id | job_id |
| 0x72 | ADMIN_LIST_PENDING_RESOLUTIONS | (none) | list of ambiguity audits |
| 0x73 | ADMIN_RESOLVE_AMBIGUITY | audit_id, chosen_entity | confirmation |
| 0x74 | ADMIN_GET_AUDIT | audit_id | AuditEntry |
| 0x75 | ADMIN_LIST_STALE_STATEMENTS | filter | StatementIds |
| 0x76 | ADMIN_BACKFILL | extractor_ids, memory_range | job_id |
| 0x77 | ADMIN_JOB_STATUS | job_id | status, progress, ETA |

## Body encoding

All bodies use rkyv (zero-copy deserialization), the same encoding used for cognitive primitives. Variable-length fields use rkyv length-prefixed encoding. CRC32C on the body.

## Streaming responses

Large query results stream:
- Server sends a `STREAM_START` frame with metadata.
- Multiple `STREAM_ITEM` frames carry results.
- A `STREAM_END` frame finalizes.

Client can cancel mid-stream by sending `STREAM_CANCEL` with the request_id.

## SUBSCRIBE event types

The SUBSCRIBE primitive (section 03) carries event types for the knowledge layer:

- `ENTITY_CREATED`, `ENTITY_UPDATED`, `ENTITY_MERGED`, `ENTITY_RENAMED`
- `STATEMENT_CREATED`, `STATEMENT_SUPERSEDED`, `STATEMENT_TOMBSTONED`
- `RELATION_CREATED`, `RELATION_SUPERSEDED`
- `EXTRACTION_COMPLETED` (with extractor_id, memory_id, output_count)
- `EXTRACTION_FAILED` (with extractor_id, memory_id, error)
- `SCHEMA_UPDATED` (with from_version, to_version)

Subscribers filter by event type, entity_id, predicate, etc.

## Error codes

Beyond the substrate's error codes (section 03):

| Code | Meaning |
|---|---|
| 0x20 | SCHEMA_INVALID |
| 0x21 | SCHEMA_MIGRATION_REQUIRED |
| 0x30 | ENTITY_NOT_FOUND |
| 0x31 | ENTITY_TYPE_MISMATCH |
| 0x32 | ENTITY_AMBIGUOUS |
| 0x33 | ENTITY_MERGE_CONFLICT |
| 0x40 | STATEMENT_NOT_FOUND |
| 0x41 | STATEMENT_OBJECT_TYPE_MISMATCH |
| 0x42 | STATEMENT_CONTRADICTS_EXISTING |
| 0x50 | RELATION_CARDINALITY_VIOLATION |
| 0x60 | QUERY_TIMEOUT |
| 0x61 | QUERY_OVER_BUDGET |
| 0x70 | EXTRACTOR_DISABLED |
| 0x71 | EXTRACTOR_BUDGET_EXCEEDED |
| 0x72 | EXTRACTION_FAILED |

Error responses include human-readable detail in the body.

## Schema-optional behavior

The server operates with or without a declared schema:

- **No schema declared**: opcodes 0x20–0x77 return `SCHEMA_NOT_DECLARED` errors except for SCHEMA_UPLOAD itself. Substrate primitives (0x10–0x1F) work normally.
- **Schema declared**: all opcodes function. RECALL routes through hybrid retrieval.

This is a deployment choice, not a compatibility mode. A deployment that wants only the vector substrate simply doesn't declare a schema.
