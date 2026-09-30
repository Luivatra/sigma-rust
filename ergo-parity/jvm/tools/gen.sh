#!/usr/bin/env bash
# Produce SCHEMA.md fixtures from the node with ergo-core's own JSON decoders.
#   tools/gen.sh --node http://192.168.1.137:9053 --from 700000 --to 700050 --out fixtures.jsonl
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARITY_MAIN=parity.FixtureGen exec "$here/run.sh" "$@"
