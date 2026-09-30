# ergo-parity

Differential harness: does sigma-rust validate mainnet transactions the way the
Ergo node does? Formats and conventions are in [SCHEMA.md](SCHEMA.md). The JVM
runner lives in `jvm/`.

Build with the speed-optimised profile (the workspace `release` profile is
size-optimised with LTO, which makes evaluation slow):

    cargo build --profile parity -p ergo-parity
    P=target/parity/ergo-parity

## extract

    $P extract --node http://192.168.1.137:9053 --heights 700000..700050 \
        --out fixtures.jsonl --rejects rejects.jsonl [--concurrency 8] \
        [--cache ergo-parity-cache | --no-cache]

Heights are inclusive. Node responses are cached on disk (default
`ergo-parity-cache/`; delete it after a reorg). Fixtures that fail an id check
go to the reject log with a `reason` (`tx_id`, `box_id`, `header_id`,
`header_window`, `epoch_parameters`, `block_lookup`, ...). Rejects are also
printed to stderr, and a per-reason count is printed at the end.

## eval

    $P eval --in fixtures.jsonl --out rust.jsonl

Runs stateless then stateful validation with this workspace's ergo-lib, in
parallel. Panics become `status: error` with an error starting `panic:`.

## diff

    $P diff rust.jsonl jvm.jsonl [--limit 50]

Prints the first `--limit` mismatches and a count per kind (`status`,
`error_class`, `total_cost`, `input_count`, `input_ok`, `input_cost`,
`only_in_a`, `only_in_b`). Exit code 1 on any mismatch, 2 on a usage or I/O
error.

## JVM side

    jvm/run.sh --in fixtures.jsonl --out jvm.jsonl

## Tests

    cargo test -p ergo-parity
