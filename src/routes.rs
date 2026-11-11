use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

const EXCLUDED_TAGS: [&str; 5] = ["Distributed", "Service", "Snapshots", "Beta", "Quotas"];
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

static EXCLUDED_PATHS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^/(cluster|telemetry|metrics|healthz|livez|readyz|snapshots|issues)(/.*)?$",
        r"^/collections/\{[^}]+\}/(cluster|shards|snapshots)(/.*)?$",
        r"^/$",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("Exclusion regex should compile"))
    .collect()
});

// Qdrant uses POST for queries with a body, so these are readers.
static READER_POSTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^/collections/\{[^}]+\}/",
        r"(points|points/scroll|points/count|facet|points/query|points/query/batch",
        r"|points/query/groups|points/search/matrix/(pairs|offsets))$"
    ))
    .expect("Reader regex should compile")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Exclude,
    Reader,
    Writer,
}

impl Route {
    pub fn label(self) -> &'static str {
        match self {
            Route::Writer => "writer",
            _ => "reader",
        }
    }
}

pub fn classify(method: &str, path: &str, tags: &[&str]) -> Route {
    if tags.iter().any(|t| EXCLUDED_TAGS.contains(t))
        || EXCLUDED_PATHS.iter().any(|re| re.is_match(path))
    {
        return Route::Exclude;
    }
    match method.to_ascii_uppercase().as_str() {
        "POST" if READER_POSTS.is_match(path) => Route::Reader,
        "POST" | "PUT" | "PATCH" | "DELETE" => Route::Writer,
        "GET" => Route::Reader,
        _ => Route::Exclude,
    }
}

/// Removes the operations that must not become tools. When writes are off the
/// writer operations are removed too, so those tools never exist.
pub fn prune_spec(spec: &mut Value, enable_writes: bool) {
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    paths.retain(|path, item| {
        let Some(item) = item.as_object_mut() else {
            return true;
        };
        item.retain(|key, operation| {
            if !METHODS.contains(&key.as_str()) {
                return true;
            }
            let tags: Vec<&str> = operation
                .get("tags")
                .and_then(Value::as_array)
                .map(|tags| tags.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            match classify(key, path, &tags) {
                Route::Exclude => false,
                Route::Writer => enable_writes,
                Route::Reader => true,
            }
        });
        item.keys().any(|key| METHODS.contains(&key.as_str()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn excluded_by_tag_or_path() {
        assert_eq!(
            classify("GET", "/cluster", &["Distributed"]),
            Route::Exclude
        );
        assert_eq!(classify("GET", "/healthz", &["Service"]), Route::Exclude);
        assert_eq!(classify("GET", "/issues", &["Beta"]), Route::Exclude);
        // path fallback when the tags are missing
        assert_eq!(classify("GET", "/readyz", &[]), Route::Exclude);
        assert_eq!(classify("GET", "/", &[]), Route::Exclude);
        assert_eq!(
            classify("GET", "/snapshots/{snapshot_name}", &[]),
            Route::Exclude
        );
        assert_eq!(
            classify("POST", "/collections/{collection_name}/shards", &[]),
            Route::Exclude
        );
        assert_eq!(
            classify(
                "PUT",
                "/collections/{collection_name}/snapshots/recover",
                &[]
            ),
            Route::Exclude
        );
    }

    #[test]
    fn posts_that_only_read_are_readers() {
        for path in [
            "/collections/{collection_name}/points",
            "/collections/{collection_name}/points/scroll",
            "/collections/{collection_name}/points/count",
            "/collections/{collection_name}/facet",
            "/collections/{collection_name}/points/query",
            "/collections/{collection_name}/points/query/batch",
            "/collections/{collection_name}/points/query/groups",
            "/collections/{collection_name}/points/search/matrix/pairs",
            "/collections/{collection_name}/points/search/matrix/offsets",
        ] {
            assert_eq!(classify("POST", path, &["Points"]), Route::Reader, "{path}");
        }
    }

    #[test]
    fn everything_else_that_mutates_is_a_writer() {
        assert_eq!(
            classify("PUT", "/collections/{collection_name}/points", &[]),
            Route::Writer
        );
        assert_eq!(
            classify("POST", "/collections/{collection_name}/points/delete", &[]),
            Route::Writer
        );
        assert_eq!(
            classify("DELETE", "/collections/{collection_name}", &[]),
            Route::Writer
        );
        assert_eq!(
            classify("PATCH", "/collections/{collection_name}", &[]),
            Route::Writer
        );
        assert_eq!(
            classify("GET", "/collections", &["Collections"]),
            Route::Reader
        );
        assert_eq!(classify("HEAD", "/collections", &[]), Route::Exclude);
    }

    fn spec() -> Value {
        json!({
            "paths": {
                "/healthz": { "get": { "operationId": "healthz", "tags": ["Service"] } },
                "/collections": { "get": { "operationId": "get_collections", "tags": ["Collections"] } },
                "/collections/{collection_name}": {
                    "parameters": [{ "name": "collection_name", "in": "path" }],
                    "delete": { "operationId": "delete_collection", "tags": ["Collections"] }
                },
                "/collections/{collection_name}/points": {
                    "put": { "operationId": "upsert_points", "tags": ["Points"] },
                    "post": { "operationId": "get_points", "tags": ["Points"] }
                },
                "/collections/{collection_name}/snapshots": {
                    "post": { "operationId": "create_snapshot", "tags": ["Snapshots"] }
                }
            }
        })
    }

    fn operations(spec: &Value) -> Vec<String> {
        let mut ids: Vec<String> = spec["paths"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|item| item.as_object().unwrap().values())
            .filter_map(|op| op.get("operationId").and_then(Value::as_str))
            .map(String::from)
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn read_only_keeps_readers_only() {
        let mut spec = spec();
        prune_spec(&mut spec, false);
        assert_eq!(operations(&spec), ["get_collections", "get_points"]);
        // a path left with no operations is dropped entirely
        assert!(
            spec["paths"]
                .get("/collections/{collection_name}")
                .is_none()
        );
    }

    #[test]
    fn writes_enabled_keeps_writers_but_not_ops_routes() {
        let mut spec = spec();
        prune_spec(&mut spec, true);
        assert_eq!(
            operations(&spec),
            [
                "delete_collection",
                "get_collections",
                "get_points",
                "upsert_points"
            ]
        );
        // path-level parameters survive
        assert!(spec["paths"]["/collections/{collection_name}"]["parameters"].is_array());
    }
}
