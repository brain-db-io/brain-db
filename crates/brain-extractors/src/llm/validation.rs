//! Schema-validation helper for the LLM extractor tier.
//!
//! On a schema-validation failure the extractor re-prompts the model
//! once with the validator's first error embedded in the system
//! block (per the extractor design); the second failure marks the
//! extraction as a dropped result with the error preserved in the
//! audit row.

use jsonschema::JSONSchema;
use serde_json::Value;

pub(super) fn validate_against(schema: &JSONSchema, content: &str) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(content).map_err(|e| format!("response is not valid JSON: {e}"))?;
    if let Err(mut errs) = schema.validate(&parsed) {
        let msg = match errs.next() {
            Some(e) => e.to_string(),
            None => "unknown validation failure".into(),
        };
        return Err(msg);
    }
    Ok(parsed)
}
