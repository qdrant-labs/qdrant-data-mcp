use serde_json::{Value, json};
use url::Url;

pub const LOCAL_PREFIX: &str = "local/";

#[derive(Debug, Clone)]
pub struct EmbeddingsConfig {
    pub url: Url,
    pub api_key: Option<String>,
}

impl EmbeddingsConfig {
    pub fn resolve(url: Option<String>, api_key: Option<String>) -> Result<Option<Self>, String> {
        let Some(url) = url.filter(|url| !url.is_empty()) else {
            return Ok(None);
        };
        let parsed = Url::parse(&url)
            .ok()
            .filter(|parsed| matches!(parsed.scheme(), "http" | "https"))
            .ok_or_else(|| format!("--embeddings-url must be an http(s) URL, got \"{url}\""))?;
        Ok(Some(Self {
            url: parsed,
            api_key: api_key.filter(|key| !key.is_empty()),
        }))
    }

    fn endpoint(&self) -> String {
        format!("{}/embeddings", self.url.as_str().trim_end_matches('/'))
    }
}

struct LocalDoc {
    pointer: String,
    model: String,
    text: String,
}

fn local_doc(value: &Value) -> Option<(&str, &str)> {
    let object = value.as_object()?;
    let text = object.get("text")?.as_str()?;
    let model = object.get("model")?.as_str()?.strip_prefix(LOCAL_PREFIX)?;
    Some((text, model))
}

// Finds every {"text": ..., "model": "local/<name>"} object, as a JSON pointer.
fn collect(node: &Value, pointer: &str, found: &mut Vec<LocalDoc>) {
    let children: Vec<(String, &Value)> = match node {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.replace('~', "~0").replace('/', "~1"), value))
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        _ => return,
    };
    for (key, child) in children {
        let child_pointer = format!("{pointer}/{key}");
        match local_doc(child) {
            Some((text, model)) => found.push(LocalDoc {
                pointer: child_pointer,
                model: model.to_string(),
                text: text.to_string(),
            }),
            None => collect(child, &child_pointer, found),
        }
    }
}

/// The texts to embed with one model, and where their vectors go.
pub struct Group {
    pub model: String,
    pub texts: Vec<String>,
    pointers: Vec<String>,
}

/// Groups the `local/<model>` texts in `arguments` by model, so each model
/// needs one embeddings call.
pub fn plan(arguments: &Value) -> Vec<Group> {
    let mut docs = Vec::new();
    collect(arguments, "", &mut docs);

    let mut groups: Vec<Group> = Vec::new();
    for doc in docs {
        let position = match groups.iter().position(|group| group.model == doc.model) {
            Some(position) => position,
            None => {
                groups.push(Group {
                    model: doc.model.clone(),
                    texts: Vec::new(),
                    pointers: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[position].texts.push(doc.text);
        groups[position].pointers.push(doc.pointer);
    }
    groups
}

/// Replaces each text object of the group with its vector.
pub fn apply(arguments: &mut Value, group: &Group, vectors: Vec<Vec<f64>>) {
    for (pointer, vector) in group.pointers.iter().zip(vectors) {
        if let Some(slot) = arguments.pointer_mut(pointer) {
            *slot = Value::from(vector);
        }
    }
}

fn parse_embeddings(body: &Value, expected: usize) -> Result<Vec<Vec<f64>>, String> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Embeddings endpoint returned no data")?;
    if data.len() != expected {
        return Err(format!(
            "Embeddings endpoint returned {} vectors for {expected} inputs",
            data.len()
        ));
    }
    let mut items = Vec::with_capacity(expected);
    for (position, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Value::as_i64)
            .unwrap_or(position as i64);
        let vector: Vec<f64> = item
            .get("embedding")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if vector.is_empty() {
            return Err("Embeddings endpoint returned an empty embedding".to_string());
        }
        items.push((index, vector));
    }
    items.sort_by_key(|(index, _)| *index);
    Ok(items.into_iter().map(|(_, vector)| vector).collect())
}

pub async fn request_embeddings(
    client: &reqwest::Client,
    config: &EmbeddingsConfig,
    model: &str,
    input: &[String],
) -> Result<Vec<Vec<f64>>, String> {
    let mut request = client
        .post(config.endpoint())
        .json(&json!({ "model": model, "input": input }));
    if let Some(key) = &config.api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("Embeddings request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Embeddings request failed ({}): {body}",
            status.as_u16()
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Embeddings response was not valid JSON: {e}"))?;
    parse_embeddings(&body, input.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Embeds each text as [text length, 1], the way a real endpoint would
    // return one vector per input.
    fn rewrite_with_fake(arguments: &mut Value) -> Vec<(String, Vec<String>)> {
        let mut calls = Vec::new();
        for group in plan(arguments) {
            let vectors = group
                .texts
                .iter()
                .map(|t| vec![t.len() as f64, 1.0])
                .collect();
            apply(arguments, &group, vectors);
            calls.push((group.model.clone(), group.texts.clone()));
        }
        calls
    }

    #[test]
    fn replaces_local_texts_in_an_upsert() {
        let mut args = json!({
            "collection_name": "c",
            "request_body": { "points": [
                { "id": 1, "vector": { "dense": { "text": "abc", "model": "local/m" } } },
                { "id": 2, "vector": { "text": "hello", "model": "local/m" } },
            ]}
        });
        let calls = rewrite_with_fake(&mut args);

        assert_eq!(
            calls,
            [(
                "m".to_string(),
                vec!["abc".to_string(), "hello".to_string()]
            )]
        );
        let points = &args["request_body"]["points"];
        assert_eq!(points[0]["vector"], json!({ "dense": [3.0, 1.0] }));
        assert_eq!(points[1]["vector"], json!([5.0, 1.0]));
    }

    #[test]
    fn replaces_queries_and_prefetches_with_one_call_per_model() {
        let mut args = json!({
            "prefetch": [{ "query": { "text": "xy", "model": "local/m" }, "using": "d" }],
            "query": { "text": "z", "model": "local/other" },
        });
        let calls = rewrite_with_fake(&mut args);

        let models: Vec<&str> = calls.iter().map(|(model, _)| model.as_str()).collect();
        assert_eq!(models, ["m", "other"]);
        assert_eq!(args["prefetch"][0]["query"], json!([2.0, 1.0]));
        assert_eq!(args["query"], json!([1.0, 1.0]));
    }

    #[test]
    fn leaves_other_models_untouched() {
        let original = json!({
            "query": { "text": "a", "model": "sentence-transformers/all-minilm-l6-v2" },
            "vector": { "text": [0.1], "model": "x" }
        });
        let mut args = original.clone();
        assert!(plan(&args).is_empty());
        assert!(rewrite_with_fake(&mut args).is_empty());
        assert_eq!(args, original);
    }

    #[test]
    fn keys_with_slashes_are_addressed_correctly() {
        let mut args = json!({ "a/b": { "c~d": { "text": "hi", "model": "local/m" } } });
        rewrite_with_fake(&mut args);
        assert_eq!(args["a/b"]["c~d"], json!([2.0, 1.0]));
    }

    #[test]
    fn embeddings_come_back_in_index_order() {
        let body = json!({ "data": [
            { "index": 1, "embedding": [2.0] },
            { "index": 0, "embedding": [1.0] },
        ]});
        assert_eq!(parse_embeddings(&body, 2).unwrap(), [[1.0], [2.0]]);
    }

    #[test]
    fn a_wrong_number_of_vectors_is_an_error() {
        let body = json!({ "data": [{ "index": 0, "embedding": [1.0] }] });
        assert!(
            parse_embeddings(&body, 2)
                .unwrap_err()
                .contains("1 vectors for 2 inputs")
        );
        assert!(parse_embeddings(&json!({}), 1).is_err());
        assert!(parse_embeddings(&json!({ "data": [{ "embedding": [] }] }), 1).is_err());
    }

    #[test]
    fn resolves_the_embeddings_url() {
        assert!(EmbeddingsConfig::resolve(None, None).unwrap().is_none());
        let config = EmbeddingsConfig::resolve(
            Some("http://localhost:11434/v1/".to_string()),
            Some("k".to_string()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.endpoint(), "http://localhost:11434/v1/embeddings");
        assert_eq!(config.api_key.as_deref(), Some("k"));
        assert!(EmbeddingsConfig::resolve(Some("not-a-url".to_string()), None).is_err());
        assert!(EmbeddingsConfig::resolve(Some("ftp://x".to_string()), None).is_err());
    }
}
