#!/bin/bash

rm -rf dist/
mkdir -p dist/

for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-apple-darwin
do
    deno compile --allow-env --allow-read --allow-net --target "$target" -o dist/qdrant-data-mcp-${target} main.ts
done
