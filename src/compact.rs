use std::{
    collections::{HashMap, hash_map::Entry},
    hash::{DefaultHasher, Hash, Hasher},
};

use serde_json::{Map, Value, json};

// A shared sub-schema is only worth a `$ref` above this size.
const MIN_BYTES: usize = 120;

// rmcp-openapi inlines every `$ref`, which makes Qdrant's schemas huge because
// the same filter and condition types repeat many times. This turns repeated
// identical sub-schemas back into `$defs` and `$ref`s.

enum Shape {
    Schemas,
    Named,
}

// How a JSON Schema keyword holds sub-schemas. Everything else (enum,
// required, default, ...) is plain data and is never replaced by a `$ref`.
fn shape(keyword: &str) -> Option<Shape> {
    match keyword {
        "items"
        | "additionalProperties"
        | "not"
        | "if"
        | "then"
        | "else"
        | "contains"
        | "propertyNames"
        | "unevaluatedItems"
        | "unevaluatedProperties"
        | "anyOf"
        | "oneOf"
        | "allOf"
        | "prefixItems" => Some(Shape::Schemas),
        "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas" => {
            Some(Shape::Named)
        }
        _ => None,
    }
}

#[derive(Default)]
struct Table<'a> {
    hashes: HashMap<*const Value, u64>,
    sizes: HashMap<u64, usize>,
    reps: HashMap<u64, &'a Value>,
    children: HashMap<u64, Vec<u64>>,
    order: Vec<u64>,
}

struct Acc {
    hasher: DefaultHasher,
    size: usize,
    kids: Vec<u64>,
}

impl<'a> Table<'a> {
    fn child(&mut self, value: &'a Value, acc: &mut Acc) {
        match value {
            Value::Object(_) => {
                if let Some((hash, size)) = self.visit(value) {
                    hash.hash(&mut acc.hasher);
                    acc.size += size;
                    acc.kids.push(hash);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| self.child(item, acc)),
            other => {
                let text = other.to_string();
                text.hash(&mut acc.hasher);
                acc.size += text.len();
            }
        }
    }

    fn visit(&mut self, node: &'a Value) -> Option<(u64, usize)> {
        let object = node.as_object()?;
        let mut acc = Acc {
            hasher: DefaultHasher::new(),
            size: 2,
            kids: Vec::new(),
        };
        let mut keys: Vec<&String> = object.keys().collect();
        keys.sort();
        for key in keys {
            let value = &object[key];
            key.hash(&mut acc.hasher);
            acc.size += key.len() + 4;
            match (shape(key), value) {
                (Some(Shape::Schemas), value) => self.child(value, &mut acc),
                (Some(Shape::Named), Value::Object(named)) => {
                    let mut names: Vec<&String> = named.keys().collect();
                    names.sort();
                    for name in names {
                        name.hash(&mut acc.hasher);
                        acc.size += name.len() + 4;
                        self.child(&named[name], &mut acc);
                    }
                }
                (_, value) => {
                    let text = value.to_string();
                    text.hash(&mut acc.hasher);
                    acc.size += text.len();
                }
            }
        }
        let hash = acc.hasher.finish();
        self.hashes.insert(node as *const Value, hash);
        if let Entry::Vacant(entry) = self.sizes.entry(hash) {
            entry.insert(acc.size);
            self.reps.insert(hash, node);
            self.children.insert(hash, acc.kids);
            self.order.push(hash);
        }
        Some((hash, acc.size))
    }

    fn emit(&self, node: &Value, names: &HashMap<u64, String>, top: bool) -> Value {
        let hash = self.hashes[&(node as *const Value)];
        if !top && let Some(name) = names.get(&hash) {
            return json!({ "$ref": format!("#/$defs/{name}") });
        }
        let mut out = Map::new();
        for (key, value) in node.as_object().expect("A schema node is an object") {
            let emitted = match (shape(key), value) {
                (Some(Shape::Schemas), value) => self.emit_value(value, names),
                (Some(Shape::Named), Value::Object(named)) => Value::Object(
                    named
                        .iter()
                        .map(|(name, schema)| (name.clone(), self.emit_value(schema, names)))
                        .collect(),
                ),
                (_, value) => value.clone(),
            };
            out.insert(key.clone(), emitted);
        }
        Value::Object(out)
    }

    fn emit_value(&self, value: &Value, names: &HashMap<u64, String>) -> Value {
        match value {
            Value::Object(_) => self.emit(value, names, false),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.emit_value(item, names))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

pub fn compact(schema: &Map<String, Value>) -> Map<String, Value> {
    let root = Value::Object(schema.clone());
    let mut table = Table::default();
    let Some((root_hash, _)) = table.visit(&root) else {
        return schema.clone();
    };

    // how many distinct parents (counting repeats) point at each sub-schema
    let mut uses: HashMap<u64, usize> = HashMap::new();
    for hash in &table.order {
        for child in &table.children[hash] {
            *uses.entry(*child).or_default() += 1;
        }
    }
    let mut names: HashMap<u64, String> = HashMap::new();
    for hash in &table.order {
        if *hash != root_hash
            && uses.get(hash).copied().unwrap_or(0) >= 2
            && table.sizes[hash] >= MIN_BYTES
        {
            names.insert(*hash, format!("d{}", names.len() + 1));
        }
    }
    if names.is_empty() {
        return schema.clone();
    }

    let mut defs = Map::new();
    for hash in &table.order {
        if let Some(name) = names.get(hash) {
            defs.insert(name.clone(), table.emit(table.reps[hash], &names, true));
        }
    }
    let Value::Object(mut result) = table.emit(&root, &names, true) else {
        unreachable!("The root is an object")
    };
    match result.get_mut("$defs") {
        Some(Value::Object(existing)) => existing.extend(defs),
        _ => {
            result.insert("$defs".to_string(), Value::Object(defs));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> Value {
        json!({
            "type": "object",
            "description": "A filter with enough text in it to be worth sharing between the places that use it",
            "properties": {
                "must": { "type": "array", "items": { "type": "string", "enum": ["a", "b", "c"] } },
                "should": { "type": "array", "items": { "type": "string", "enum": ["a", "b", "c"] } }
            },
            "required": ["must"]
        })
    }

    fn schema() -> Map<String, Value> {
        json!({
            "type": "object",
            "properties": {
                "filter": filter(),
                "prefetch": { "type": "object", "properties": { "filter": filter() } },
                "limit": { "type": "integer" }
            }
        })
        .as_object()
        .unwrap()
        .clone()
    }

    // Replaces every $ref by its definition again.
    fn inline(node: &Value, defs: &Map<String, Value>) -> Value {
        match node {
            Value::Object(map) => {
                if let Some(name) = map
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|r| r.strip_prefix("#/$defs/"))
                {
                    return inline(&defs[name], defs);
                }
                Value::Object(
                    map.iter()
                        .filter(|(key, _)| key.as_str() != "$defs")
                        .map(|(key, value)| (key.clone(), inline(value, defs)))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(|v| inline(v, defs)).collect()),
            other => other.clone(),
        }
    }

    #[test]
    fn repeated_schemas_become_defs_and_the_result_inlines_back_to_the_original() {
        let original = schema();
        let compacted = compact(&original);

        let defs = compacted["$defs"].as_object().unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(
            compacted["properties"]["filter"],
            json!({ "$ref": "#/$defs/d1" })
        );
        assert_eq!(
            compacted["properties"]["prefetch"]["properties"]["filter"],
            json!({ "$ref": "#/$defs/d1" })
        );
        assert!(
            serde_json::to_string(&compacted).unwrap().len()
                < serde_json::to_string(&original).unwrap().len()
        );

        assert_eq!(
            inline(&Value::Object(compacted.clone()), defs),
            Value::Object(original)
        );
    }

    #[test]
    fn data_is_never_replaced_by_a_ref() {
        let compacted = compact(&schema());
        let defs = compacted["$defs"].as_object().unwrap();
        let body = &defs["d1"];
        // the enum and required arrays, and the properties map, stay as they were
        assert_eq!(body["required"], json!(["must"]));
        assert_eq!(
            body["properties"]["must"]["items"]["enum"],
            json!(["a", "b", "c"])
        );
        assert!(body["properties"].get("$ref").is_none());
    }

    #[test]
    fn a_schema_without_repeats_is_unchanged() {
        let schema = json!({ "type": "object", "properties": { "a": { "type": "string" } } })
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(compact(&schema), schema);
    }
}
