# ergo-parity data formats

`ergo-parity` checks that sigma-rust validates transactions the way the Ergo
node does. The Rust extractor writes one fixture per transaction, taken from
an indexed node. Two runners evaluate the same fixtures and each write a
result per transaction: the Rust runner uses this workspace's ergo-lib, the
JVM runner uses the node's own code (ergo-core from ergo v6.1.2 with
sigma-state 6.0.2, the versions the reference node runs). `ergo-parity diff`
compares the two result files.

All files are JSON Lines: one object per line. Byte strings are lowercase hex.

## Fixture (schema 1)

```json
{
  "schema": 1,
  "height": 1884000,
  "block_id": "…",
  "tx_index": 3,
  "tx_id": "…",
  "tx": "…",
  "inputs": ["…"],
  "data_inputs": ["…"],
  "headers": ["…"],
  "header": "…",
  "parameters": {"1": 1250000, "2": 360},
  "epoch_extension": [["0001", "00000fa0"]],
  "notes": []
}
```

- `tx`: the serialized transaction, as the node would serialize it.
- `inputs`, `data_inputs`: the serialized boxes the transaction spends and
  reads, in transaction order. A box's serialization includes its transaction
  id and index.
- `headers`: the ten headers before this block, newest first, serialized.
  Fewer near genesis.
- `header`: this block's own header, serialized. Runners take the pre-header
  (version, parent id, timestamp, nBits, height, miner key, votes) from it.
- `parameters`: the blockchain parameters in force at `height`, keyed by
  parameter id (decimal string) as the node stores them in the extension of
  the epoch's first block (`height - height % 1024`).
- `epoch_extension`: every field of that epoch-start extension as
  `[key, value]`. Runners that derive validation settings (soft-fork rule
  statuses) parse them from here.
- `notes`: anything the extractor wants recorded, e.g. `"reencoded"` when a
  byte string had to be rebuilt from the node's JSON.

The node's REST API serves transactions and boxes as JSON only. The extractor
rebuilds the bytes from it. ErgoTrees, registers and proofs come through as
the original hex, so only the envelope is re-encoded. The extractor checks the
rebuilt bytes against the ids: every box against its box id, the transaction
against its tx id, every header against its header id. A fixture that fails a
check is not written. It goes to the extractor's reject log instead, with the
reason.

## Result (schema 1)

```json
{
  "schema": 1,
  "runner": "rust|jvm",
  "runner_version": "…",
  "tx_id": "…",
  "height": 1884000,
  "status": "valid|invalid|error",
  "total_cost": 12356,
  "inputs": [{"index": 0, "ok": true, "cost": 2407, "error": null}],
  "error_class": null,
  "error": null
}
```

- `status`:
  - `valid`: the transaction passes stateful validation.
  - `invalid`: the runner rejects it.
  - `error`: the runner itself failed, for example it could not parse the
    fixture. That is a harness bug, not a verdict.
- `total_cost`: the transaction's cost as the node accumulates it in
  `ErgoTransaction.statefulValidity`, in block-cost units: the initial
  transaction cost plus every input's script cost plus the token costs.
  `null` when the runner rejects before it has a cost.
- `inputs[].cost`: that input's script cost (JIT cost converted to block-cost
  units), and `null` if the input was not evaluated.
- `error_class`: one of
  - `parse`: the fixture's bytes don't parse;
  - `script_false`: a script reduced to false;
  - `proof`: a sigma proof does not verify;
  - `cost_limit`: a cost limit was exceeded;
  - `conservation`: ERG or token amounts don't balance;
  - `rule`: another validation rule failed (box size, min value, …);
  - `other`.

  `error` holds the runner's own message.

Both runners follow the node's order of checks, so these cases come out the
same on each side:

- Fixture bytes that don't parse give `status: error` with
  `error_class: parse`.
- A rejection by validation gives `status: invalid`.
- The runner itself failing (an exception, a panic, parameters that disagree
  with `epoch_extension`) gives `status: error` with `error_class: other`.
- Stateless checks run first, then the stateful ones. Inputs are verified in
  order, and the first failing input stops validation. Every input after it
  is `{"ok": false, "cost": null, "error": "not evaluated"}`.
- A failing input keeps the cost its evaluation reached. The cost is `null`
  when evaluation threw, for example on a cost limit.
- `script_false` or `proof`: the failing input's script is reduced again.
  If it reduces to false it is `script_false`, otherwise `proof`.
- `cost_limit` covers the block cost limit and the interpreter's cost limit.
  `conservation` also covers an overflow while summing amounts.
- Scripts are always evaluated. The node's mainnet checkpoint (script
  validation skipped up to height 1,231,454) is not applied.

## Diff

`ergo-parity diff <a.jsonl> <b.jsonl>` pairs results by `tx_id`. It reports:

- a mismatch in `status`, `error_class` or `total_cost`, or in any input's
  `ok` or `cost`;
- a transaction present in only one file;
- a count of each mismatch kind.

It exits non-zero on any mismatch.
