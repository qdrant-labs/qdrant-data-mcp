use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

const PREFIX: &str = "#/components/schemas/";

// rmcp-openapi rejects recursive schemas, and Qdrant has several (a Filter
// holds Conditions that can hold Filters). Walking from these first keeps
// their own structure intact and cuts the recursion deeper down.
const WALK_FIRST: [&str; 2] = ["Filter", "Prefetch"];

fn collect_refs(node: &Value, found: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix(PREFIX))
            {
                found.push(name.to_string());
            }
            map.values().for_each(|value| collect_refs(value, found));
        }
        Value::Array(items) => items.iter().for_each(|value| collect_refs(value, found)),
        _ => {}
    }
}

fn walk<'a>(
    name: &'a str,
    graph: &'a BTreeMap<String, Vec<String>>,
    state: &mut HashMap<&'a str, bool>,
    cuts: &mut Vec<(String, String)>,
) {
    state.insert(name, false);
    for next in graph.get(name).into_iter().flatten() {
        match state.get(next.as_str()) {
            Some(false) => cuts.push((name.to_string(), next.clone())),
            Some(true) => {}
            None if graph.contains_key(next) => walk(next, graph, state, cuts),
            None => {}
        }
    }
    state.insert(name, true);
}

fn replace_refs(node: &mut Value, target: &str) {
    match node {
        Value::Object(map) => {
            let is_target = map
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|r| r.strip_prefix(PREFIX) == Some(target));
            if is_target {
                *node = json!({
                    "description": format!("A nested {target}, with the same structure as above")
                });
                return;
            }
            map.values_mut()
                .for_each(|value| replace_refs(value, target));
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|value| replace_refs(value, target)),
        _ => {}
    }
}

/// Replaces the references that close a cycle between component schemas with
/// an "any value" schema, so the spec can be inlined.
pub fn break_cycles(spec: &mut Value) {
    let Some(schemas) = spec
        .pointer("/components/schemas")
        .and_then(Value::as_object)
    else {
        return;
    };
    let graph: BTreeMap<String, Vec<String>> = schemas
        .iter()
        .map(|(name, schema)| {
            let mut found = Vec::new();
            collect_refs(schema, &mut found);
            (name.clone(), found)
        })
        .collect();

    let mut cuts = Vec::new();
    let mut state = HashMap::new();
    for name in WALK_FIRST
        .iter()
        .copied()
        .filter(|name| graph.contains_key(*name))
        .chain(graph.keys().map(String::as_str))
    {
        if !state.contains_key(name) {
            walk(name, &graph, &mut state, &mut cuts);
        }
    }

    for (from, to) in cuts {
        if let Some(schema) = spec.pointer_mut(&format!("/components/schemas/{from}")) {
            replace_refs(schema, &to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_cycle(spec: &Value) -> bool {
        let schemas = spec["components"]["schemas"].as_object().unwrap();
        let graph: BTreeMap<String, Vec<String>> = schemas
            .iter()
            .map(|(name, schema)| {
                let mut found = Vec::new();
                collect_refs(schema, &mut found);
                (name.clone(), found)
            })
            .collect();
        let mut cuts = Vec::new();
        let mut state = HashMap::new();
        for name in graph.keys() {
            if !state.contains_key(name.as_str()) {
                walk(name, &graph, &mut state, &mut cuts);
            }
        }
        !cuts.is_empty()
    }

    fn spec() -> Value {
        json!({
            "components": { "schemas": {
                "Condition": { "anyOf": [
                    { "$ref": "#/components/schemas/Field" },
                    { "$ref": "#/components/schemas/Filter" }
                ]},
                "Field": { "type": "object" },
                "Filter": { "properties": {
                    "must": { "items": { "$ref": "#/components/schemas/Condition" } }
                }},
                "Prefetch": { "properties": {
                    "prefetch": { "$ref": "#/components/schemas/Prefetch" },
                    "filter": { "$ref": "#/components/schemas/Filter" }
                }}
            }}
        })
    }

    #[test]
    fn cycles_are_broken_and_the_outer_structure_is_kept() {
        let mut spec = spec();
        assert!(has_cycle(&spec));
        break_cycles(&mut spec);
        assert!(!has_cycle(&spec));

        let schemas = &spec["components"]["schemas"];
        // Filter keeps pointing at Condition; the way back from Condition is cut
        assert_eq!(
            schemas["Filter"]["properties"]["must"]["items"]["$ref"],
            "#/components/schemas/Condition"
        );
        assert_eq!(
            schemas["Condition"]["anyOf"][0]["$ref"],
            "#/components/schemas/Field"
        );
        assert!(schemas["Condition"]["anyOf"][1].get("$ref").is_none());
        assert!(
            schemas["Condition"]["anyOf"][1]["description"]
                .as_str()
                .unwrap()
                .contains("nested Filter")
        );
        // a schema that refers to itself is cut as well
        assert!(
            schemas["Prefetch"]["properties"]["prefetch"]
                .get("$ref")
                .is_none()
        );
        assert_eq!(
            schemas["Prefetch"]["properties"]["filter"]["$ref"],
            "#/components/schemas/Filter"
        );
    }

    #[test]
    fn a_spec_without_cycles_is_unchanged() {
        let original = json!({ "components": { "schemas": {
            "A": { "$ref": "#/components/schemas/B" },
            "B": { "type": "string" }
        }}});
        let mut spec = original.clone();
        break_cycles(&mut spec);
        assert_eq!(spec, original);
        let mut no_components = json!({ "paths": {} });
        break_cycles(&mut no_components);
    }
}
