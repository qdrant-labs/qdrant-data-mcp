# Using the Qdrant data tools

This server exposes a Qdrant vector search engine as tools.

## First steps

1. Call `get_collections` to see which collections exist.
2. Call `get_collection` for the one you want. It lists each vector's name, size, and distance, and which payload fields are indexed. Use those exact names and sizes in every later call.

## Calling tools

- Path and query parameters are top-level arguments, such as `collection_name`.
- The JSON body of a request goes under one argument, `request_body`.
- A result has the HTTP `status` and the response `body`. Check `status` before you use `body`.
- `query_points`, `scroll_points`, `count_points`, `facet`, and the other search tools only read data, even though a client may label them as not read-only.

A search looks like this:

```json
{
  "collection_name": "articles",
  "request_body": {
    "using": "dense",
    "query": [0.1, 0.2, 0.3],
    "limit": 5,
    "with_payload": true
  }
}
```

## Filters

Filters go in `request_body.filter`. The tool schema describes a nested filter only as "any value", but the syntax is Qdrant's own, for example:

```json
{ "must": [{ "key": "city", "match": { "value": "London" } }] }
```

Filtering on a payload field that isn't indexed may be rejected. The `payload_schema` from `get_collection` shows which fields are indexed.

## Text instead of vectors

Where a vector is expected, in both upserts and queries, you can pass a text object instead: `{"text": "...", "model": "<model name>"}`. Image models take `{"image": "<url or base64>", "model": "..."}`. Use the same model for the points you store and the queries you run, and make sure the collection's vector size matches the model's output size.

### Qdrant Cloud Inference

Every _managed_ Qdrant Cloud cluster has Cloud Inference (QCI), which embeds those objects for you. Clusters differ only in which models they can use: a free cluster can use the free models, and a paid cluster can use the free and the paid ones. The cluster's Inference tab in the Qdrant Cloud console lists the models, their output sizes, and their cost, so if you don't know which model to use, ask the user. For example, `sentence-transformers/all-minilm-l6-v2` produces 384 dimensions, and `qdrant/bm25` produces sparse vectors for keyword search (create the sparse vector with the `idf` modifier). A self-hosted Qdrant has no Cloud Inference.
