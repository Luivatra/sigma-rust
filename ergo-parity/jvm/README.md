# ergo-parity JVM reference runner

Validates each fixture (see `../SCHEMA.md`) with the Ergo node's own code and writes one result per line.
`runner_version` is `ergo-core 6.1.2/sigma-state 6.0.2`.

## Run

    ./run.sh --in fixtures.jsonl --out jvm.jsonl [--threads N]

The first call builds with sbt and caches the classpath in `target/classpath.txt`; a later source change triggers a
rebuild. Environment: `PARITY_JAVA_HOME` (default `/usr/lib/jvm/java-17-openjdk`), `PARITY_XMX` (default 4g).

## Artifacts

| Artifact | Source |
|---|---|
| `org.scorexfoundation:sigma-state_2.12:6.0.2` | Maven Central |
| `org.ergoplatform:ergo-core_2.12:6.1.2` | built locally from ergo tag `v6.1.2` (commit `7086c6c75`), `~/.ivy2/local` |
| `org.ergoplatform:avldb_2.12:6.1.2`, `ergo-wallet_2.12:6.1.2` | same build (ergo-core depends on them; `ErgoInterpreter` and `ErgoBoxSerializer` live in ergo-wallet) |

ergo-core 6.1.2 is not on Central or Sonatype releases. To rebuild it:

    git -C /home/luivatra/develop/ergo/ergo worktree add --detach /home/luivatra/develop/ergo/sigma-rust-wt/ergo-v6.1.2 v6.1.2
    cd /home/luivatra/develop/ergo/sigma-rust-wt/ergo-v6.1.2
    JAVA_HOME=/usr/lib/jvm/java-17-openjdk sbt avldb/publishLocal ergoWallet/publishLocal ergoCore/publishLocal

Scala 2.12.20 and sigma-state 6.0.2 match `build.sbt` at `v6.1.2`. JDK: Arch `java-17-openjdk` (JDK 26 is the system
default and is not used).

`src/main/resources/mainnet.conf` and `ergo-application.conf` are copies of the node's `mainnet.conf` and
`application.conf` at v6.1.2. `Chain.scala` reads `ergo.chain` from them with the node's own `ChainSettings` readers.

## What it mirrors

`ErgoState.execTransactions` (UtxoState.applyTransactions) per transaction:

1. `tx.validateStateless()`;
2. `tx.validateStateful(boxesToSpend, dataBoxes, stateContext, accumulatedCost = 0)(verifier)`, which is what
   `statefulValidity` wraps. The cost limit is the node's: `maxBlockCost` from the parameters, minus the cost so far,
   passed to each input's `ErgoContext`.

State context, as `ErgoStateContext.process` builds it for the first-and-later blocks of an epoch:
`lastHeaders = header +: headers.take(9)` (the block's own header is the pre-header, the 9 before it fill
`LastHeaders`), parameters from `Parameters.parseExtension(epochStart, extension)`, validation settings from
`ErgoValidationSettings.parseExtension(extension)`, both from `epoch_extension`. For 6.0 blocks the default
sub-blocks-per-block parameter is added when missing, as the node does. The fixture's `parameters` map is only
cross-checked against the parsed values; a disagreement is a `status: error` (harness bug).

The node's mainnet checkpoint (script validation skipped up to height 1231454) is not applied: the runner always
runs the scripts. Because the runner validates one transaction against its own boxes, txs are not checked against a
UTXO set or in-block ordering.

## Per-input cost

`RecordingInterpreter` subclasses the node's `ErgoInterpreter` and records the result of every `verify` call while
`validateStateful` runs. `validateStateful` calls `verifier.verify` once per input, in order, and sums the returned
costs. The recorded cost is that value, so there is no second evaluation.
`total_cost` is the value `validateStateful` returns (initial cost + token access cost + input costs, block-cost
units, accumulated cost 0).

Inputs the node did not reach (it stops at the first failure) are written as `ok: false, cost: null,
error: "not evaluated"`. A failed input has its recorded cost (`cost` is the JIT cost, converted, at the point of
failure) or `null` when the interpreter threw (e.g. cost limit exceeded).

## Status and error_class

- fixture bytes or context that do not parse: `status: error`, `error_class: parse`;
- the node's validation rejects: `status: invalid`, class from the rule text: `script_false` / `proof` (input failed;
  the script is re-reduced with the node's interpreter to tell them apart), `cost_limit` (`bsBlockTransactionsCost`,
  or `CostLimitException` inside an input), `conservation` (ERG/token balance and sum overflows), `rule` (other
  rules), `other` (interpreter exceptions, anything else);
- the runner itself throwing: `status: error`, `error_class: other`.

## Fixture generator (cross-check for the Rust extractor)

    tools/gen.sh --node http://192.168.1.137:9053 --from 700000 --to 700050 --out fixtures.jsonl

`tools/FixtureGen.scala` rebuilds bytes from the node's REST JSON with ergo-core's own decoders and serializers, and
checks header, transaction and box ids. It lists every fixture with `notes: ["reencoded", "jvm-fixturegen"]`.
Boxes created earlier in the same block are taken from the block, the rest from `/blockchain/box/byId`.

## Verification (2026-09-30)

Heights 700000..700050: 276 transactions, all `valid`. Against
`harness/ergo-lib/tests/test-vectors/tx_costs_700000_700050.json` (127 transactions, Scala `block_cost`), `total_cost`
matches on 127 of 127. Negative cases (wrong input boxes, missing box, truncated bytes, lowered `maxBlockCost`)
produce `proof`, `rule`, `conservation`, `parse` and `cost_limit` results.
