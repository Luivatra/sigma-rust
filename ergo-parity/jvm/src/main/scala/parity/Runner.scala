package parity

import io.circe.{Json, parser}
import org.ergoplatform.{ErgoBox, ErgoLikeContext}
import org.ergoplatform.modifiers.history.extension.Extension
import org.ergoplatform.modifiers.history.header.{Header, HeaderSerializer}
import org.ergoplatform.modifiers.mempool.{ErgoTransaction, ErgoTransactionSerializer}
import org.ergoplatform.nodeView.state.{ErgoStateContext, VotingData}
import org.ergoplatform.settings.{ErgoValidationSettings, Parameters}
import org.ergoplatform.validation.ValidationResult
import org.ergoplatform.wallet.boxes.ErgoBoxSerializer
import org.ergoplatform.wallet.interpreter.ErgoInterpreter
import scorex.util.ModifierId
import sigma.ast.ErgoTree
import sigmastate.interpreter.Interpreter.{ScriptEnv, VerificationResult}

import java.io.{BufferedReader, BufferedWriter, FileReader, FileWriter}
import java.util.concurrent.{Callable, Executors, ThreadFactory}
import scala.collection.JavaConverters._
import scala.collection.mutable
import scala.util.{Failure, Success, Try}

/** Raised when a fixture's bytes or context cannot be built (schema error_class `parse`). */
final class FixtureParseError(msg: String, cause: Throwable = null) extends Exception(msg, cause)

/** What the node's interpreter did for one input, recorded while statefulValidity runs. */
final case class InputRecord(index: Int, result: Try[VerificationResult], reducedToFalse: Option[Boolean])

/**
  * The node's ErgoInterpreter with a recorder around `verify`. ErgoTransaction.validateStateful calls
  * `verifier.verify(box.ergoTree, ctx, proof, messageToSign)` once per input, in order, and folds the returned
  * cost into the transaction cost. Recording that call keeps the node's own code as the only evaluator, so
  * per-input costs are exactly the values the node summed.
  */
final class RecordingInterpreter(params: Parameters) extends ErgoInterpreter(params) {
  val records: mutable.ArrayBuffer[InputRecord] = mutable.ArrayBuffer.empty

  override def verify(env: ScriptEnv,
                      exp: ErgoTree,
                      context: CTX,
                      proof: Array[Byte],
                      message: Array[Byte]): Try[VerificationResult] = {
    val res = super.verify(env, exp, context, proof, message)
    records += InputRecord(context.selfIndex, res, None)
    res
  }
}

object Runner {
  val RunnerVersion = "ergo-core 6.1.2/sigma-state 6.0.2"

  private val chain = Chain.mainnet

  // ---- fixture -> node objects -------------------------------------------------------------------------------

  private def parsed[T](what: String)(f: => T): T =
    Try(f) match {
      case Success(v) => v
      case Failure(e) => throw new FixtureParseError(s"$what: ${e.getClass.getSimpleName}: ${e.getMessage}", e)
    }

  final case class Fixture(height: Int,
                           txId: String,
                           tx: ErgoTransaction,
                           inputs: IndexedSeq[ErgoBox],
                           dataInputs: IndexedSeq[ErgoBox],
                           ctx: ErgoStateContext)

  def buildContext(j: io.circe.HCursor): (ErgoStateContext, Int) = {
    val height = j.downField("height").as[Int].fold(throw _, identity)
    val headers = parsed("headers") {
      j.downField("headers").as[List[String]].fold(throw _, identity)
        .map(h => HeaderSerializer.parseBytes(Hex.decode(h)))
    }
    val header = parsed("header") {
      HeaderSerializer.parseBytes(Hex.decode(j.downField("header").as[String].fold(throw _, identity)))
    }
    val epochFields = parsed("epoch_extension") {
      j.downField("epoch_extension").as[List[(String, String)]].fold(throw _, identity)
        .map { case (k, v) => (Hex.decode(k), Hex.decode(v)) }
    }
    val fixtureParams = parsed("parameters") {
      j.downField("parameters").as[Map[String, Int]].fold(throw _, identity)
    }

    val votingLength = chain.voting.votingLength
    val epochStart = height - height % votingLength
    // Same derivation as ErgoStateContext.processExtension for the first block of an epoch: parameters and
    // validation settings are parsed from that block's extension and stay in force for the whole epoch.
    val extension = Extension(ModifierId @@ ("00" * 32), epochFields)
    val parsedParams = parsed("parameters from epoch_extension") { Parameters.parseExtension(epochStart, extension).get }
    val params =
      if (header.version >= Header.Interpreter60Version &&
        !parsedParams.parametersTable.contains(Parameters.SubblocksPerBlockIncrease)) {
        // node: 6.0 inserts the default number of sub-blocks per block when the extension lacks it
        parsedParams.withNumOfSubblocksPerBlock(Parameters.SubblocksPerBlockDefault)
      } else parsedParams
    fixtureParams.foreach { case (k, v) =>
      val id = k.toInt
      if (id != Parameters.SoftForkDisablingRules && !parsedParams.parametersTable.get(id.toByte).contains(v))
        throw new FixtureParseError(s"parameters[$k]=$v disagrees with epoch_extension (${parsedParams.parametersTable.get(id.toByte)})")
    }
    val vSettings = parsed("validation settings from epoch_extension") { ErgoValidationSettings.parseExtension(extension).get }

    // ErgoStateContext.process: newHeaders = header +: lastHeaders.take(Constants.LastHeadersInContext - 1)
    val lastHeaders = header +: headers.take(9)
    val ctx = new ErgoStateContext(lastHeaders, None, chain.genesisStateDigest, params, vSettings, VotingData.empty)(chain)
    (ctx, height)
  }

  def parseFixture(line: String): Fixture = {
    val json = parser.parse(line).fold(e => throw new FixtureParseError(s"bad JSON: ${e.getMessage}", e), identity)
    val c = json.hcursor
    val txId = c.downField("tx_id").as[String].fold(_ => "", identity)
    val tx = parsed("tx") {
      ErgoTransactionSerializer.parseBytes(Hex.decode(c.downField("tx").as[String].fold(throw _, identity)))
    }
    def boxes(field: String) = parsed(field) {
      c.downField(field).as[List[String]].fold(throw _, identity)
        .map(b => ErgoBoxSerializer.parseBytes(Hex.decode(b))).toIndexedSeq
    }
    val inputs = boxes("inputs")
    val dataInputs = boxes("data_inputs")
    val (ctx, height) = buildContext(c)
    Fixture(height, txId, tx, inputs, dataInputs, ctx)
  }

  // ---- validation ----------------------------------------------------------------------------------------------

  /** Maps the node's validation failure text to the schema's error_class. */
  def classify(msg: String, records: Seq[InputRecord]): String = {
    def has(s: String) = msg.contains(s)
    if (has("Scripts of all transaction inputs should pass verification")) {
      records.lastOption match {
        case Some(InputRecord(_, Failure(e), _)) if isCostLimit(e) => "cost_limit"
        case Some(InputRecord(_, Failure(_), _)) => "other"
        case Some(InputRecord(_, Success((_, _)), Some(true))) => "script_false"
        case Some(InputRecord(_, Success((_, _)), Some(false))) => "proof"
        case _ => "other"
      }
    }
    else if (has("Accumulated cost of block transactions should not exceed")) "cost_limit"
    else if (has("Amount of Ergs in inputs should be equal") ||
      has("For every token, its amount in outputs should not exceed") ||
      has("Sum of transaction inputs should not exceed") ||
      has("Sum of transaction output values should not exceed")) "conservation"
    else if (has("Validation errors:") || has("Every ") || has("A ") || has("Box ") || has("Transaction ") ||
      has("Creation height") || has("All token amounts") || has("There should be no duplicate")) "rule"
    else "other"
  }

  private def isCostLimit(e: Throwable): Boolean = {
    def walk(t: Throwable, depth: Int): Boolean =
      t != null && depth < 6 &&
        (t.getClass.getName.contains("CostLimitException") || Option(t.getMessage).exists(_.contains("Estimated execution cost")) ||
          walk(t.getCause, depth + 1))
    walk(e, 0)
  }

  private def costLimitMsg(e: Throwable) = e.getClass.getSimpleName + ": " + e.getMessage

  private def inputsJson(f: Fixture, records: Seq[InputRecord]): Json = {
    val byIndex = records.map(r => r.index -> r).toMap
    Json.arr(f.tx.inputs.indices.map { i =>
      byIndex.get(i) match {
        case Some(InputRecord(_, Success((ok, cost)), _)) =>
          Json.obj("index" -> Json.fromInt(i), "ok" -> Json.fromBoolean(ok), "cost" -> Json.fromLong(cost),
            "error" -> (if (ok) Json.Null else Json.fromString("script did not verify")))
        case Some(InputRecord(_, Failure(e), _)) =>
          Json.obj("index" -> Json.fromInt(i), "ok" -> Json.False, "cost" -> Json.Null,
            "error" -> Json.fromString(costLimitMsg(e)))
        case None =>
          Json.obj("index" -> Json.fromInt(i), "ok" -> Json.False, "cost" -> Json.Null,
            "error" -> Json.fromString("not evaluated"))
      }
    }: _*)
  }

  private def result(f: Fixture, txIdOverride: Option[String], status: String, totalCost: Option[Int], inputs: Json,
                     errorClass: Option[String], error: Option[String]): Json =
    Json.obj(
      "schema" -> Json.fromInt(1),
      "runner" -> Json.fromString("jvm"),
      "runner_version" -> Json.fromString(RunnerVersion),
      "tx_id" -> Json.fromString(txIdOverride.getOrElse(f.tx.id)),
      "height" -> Json.fromInt(f.height),
      "status" -> Json.fromString(status),
      "total_cost" -> totalCost.map(Json.fromInt).getOrElse(Json.Null),
      "inputs" -> inputs,
      "error_class" -> errorClass.map(Json.fromString).getOrElse(Json.Null),
      "error" -> error.map(Json.fromString).getOrElse(Json.Null))

  /** Mirrors ErgoState.execTransactions for one transaction: validateStateless, then validateStateful with the
    * node's interpreter. accumulatedCost is 0 because the schema defines total_cost per transaction. */
  def validate(f: Fixture): Json = {
    val verifier = new RecordingInterpreter(f.ctx.currentParameters)
    val stateless = f.tx.validateStateless().result
    val res: ValidationResult[Any] =
      if (!stateless.isValid) stateless
      else f.tx.validateStateful(f.inputs, f.dataInputs, f.ctx, 0L)(verifier).result
    val records = verifier.records.toList
    res match {
      case ValidationResult.Valid(cost: Long) =>
        result(f, None, "valid", Some(cost.toInt), inputsJson(f, records), None, None)
      case _: ValidationResult.Valid[_] =>
        result(f, None, "invalid", None, inputsJson(f, records), Some("other"), Some("stateless check returned no cost"))
      case inv: ValidationResult.Invalid =>
        // annotate the last failing input: is the script false, or does the proof not verify?
        val annotated = records.lastOption match {
          case Some(r @ InputRecord(idx, Success((false, _)), _)) =>
            records.init :+ r.copy(reducedToFalse = reducedToFalse(f, verifier, idx))
          case _ => records
        }
        val msg = inv.message
        result(f, None, "invalid", None, inputsJson(f, annotated), Some(classify(msg, annotated)), Some(msg))
    }
  }

  /** After a failed input, re-reduce its script with the node's interpreter to see whether it reduced to false
    * (script_false) or to a sigma proposition the proof does not satisfy (proof). */
  private def reducedToFalse(f: Fixture, verifier: ErgoInterpreter, idx: Int): Option[Boolean] = Try {
    import org.ergoplatform.nodeView.ErgoContext
    import org.ergoplatform.sdk.wallet.protocol.context.TransactionContext
    import org.ergoplatform.wallet.protocol.context.InputContext
    val box = f.inputs(idx)
    val proof = f.tx.inputs(idx).spendingProof
    val ectx = new ErgoContext(f.ctx, TransactionContext(f.inputs, f.dataInputs, f.tx),
      InputContext(idx.toShort, proof.extension),
      costLimit = f.ctx.currentParameters.maxBlockCost.toLong, initCost = 0)
    val red = verifier.fullReduction(box.ergoTree, ectx, Map.empty)
    red.value == sigma.data.TrivialProp.FalseProp
  }.toOption

  def runLine(line: String): String = {
    val out: Json =
      Try(parseFixture(line)) match {
        case Failure(e) =>
          val (cls, st) = e match {
            case _: FixtureParseError => (Some("parse"), "error")
            case _ => (Some("other"), "error")
          }
          val txId = parser.parse(line).toOption.flatMap(_.hcursor.downField("tx_id").as[String].toOption).getOrElse("")
          val height = parser.parse(line).toOption.flatMap(_.hcursor.downField("height").as[Int].toOption).getOrElse(0)
          Json.obj(
            "schema" -> Json.fromInt(1), "runner" -> Json.fromString("jvm"),
            "runner_version" -> Json.fromString(RunnerVersion), "tx_id" -> Json.fromString(txId),
            "height" -> Json.fromInt(height), "status" -> Json.fromString(st), "total_cost" -> Json.Null,
            "inputs" -> Json.arr(), "error_class" -> cls.map(Json.fromString).getOrElse(Json.Null),
            "error" -> Json.fromString(Option(e.getMessage).getOrElse(e.toString)))
        case Success(f) =>
          Try(validate(f)) match {
            case Success(j) => j
            case Failure(e) =>
              // The node's code threw instead of returning a validation result. That is the node rejecting the
              // transaction in a way it does not model as a rule (e.g. arithmetic overflow while summing costs).
              result(f, None, "invalid", None, inputsJson(f, Nil), Some("other"),
                Some(e.getClass.getSimpleName + ": " + e.getMessage))
          }
      }
    out.noSpaces
  }

  // ---- main ------------------------------------------------------------------------------------------------------

  def main(args: Array[String]): Unit = {
    var in: Option[String] = None
    var out: Option[String] = None
    var threads = Runtime.getRuntime.availableProcessors()
    var i = 0
    while (i < args.length) {
      args(i) match {
        case "--in" => in = Some(args(i + 1)); i += 2
        case "--out" => out = Some(args(i + 1)); i += 2
        case "--threads" => threads = args(i + 1).toInt; i += 2
        case other => System.err.println(s"unknown argument $other"); sys.exit(2)
      }
    }
    if (in.isEmpty || out.isEmpty) {
      System.err.println("usage: Runner --in fixtures.jsonl --out results.jsonl [--threads N]")
      sys.exit(2)
    }
    val pool = Executors.newFixedThreadPool(threads, new ThreadFactory {
      def newThread(r: Runnable): Thread = {
        val t = new Thread(null, r, "parity-worker", 64L * 1024 * 1024) // node runs with -Xss64m
        t.setDaemon(true)
        t
      }
    })
    val reader = new BufferedReader(new FileReader(in.get))
    val writer = new BufferedWriter(new FileWriter(out.get))
    val batchSize = threads * 16
    var done = 0L
    var valid, invalid, error = 0L
    try {
      var eof = false
      while (!eof) {
        val batch = mutable.ArrayBuffer.empty[String]
        while (batch.size < batchSize && !eof) {
          val l = reader.readLine()
          if (l == null) eof = true else if (l.trim.nonEmpty) batch += l
        }
        val futures = batch.map { l =>
          pool.submit(new Callable[String] {
            def call(): String =
              try runLine(l)
              catch {
                case t: Throwable =>
                  Json.obj("schema" -> Json.fromInt(1), "runner" -> Json.fromString("jvm"),
                    "runner_version" -> Json.fromString(RunnerVersion), "tx_id" -> Json.fromString(""),
                    "height" -> Json.fromInt(0), "status" -> Json.fromString("error"), "total_cost" -> Json.Null,
                    "inputs" -> Json.arr(), "error_class" -> Json.fromString("other"),
                    "error" -> Json.fromString(t.toString)).noSpaces
              }
          })
        }
        futures.foreach { fu =>
          val s = fu.get()
          writer.write(s); writer.write('\n')
          if (s.contains("\"status\":\"valid\"")) valid += 1
          else if (s.contains("\"status\":\"invalid\"")) invalid += 1
          else error += 1
          done += 1
        }
        writer.flush()
        if (done % 1000 < batch.size) System.err.println(s"[parity-jvm] $done done (valid $valid, invalid $invalid, error $error)")
      }
    } finally {
      writer.close(); reader.close(); pool.shutdown()
    }
    System.err.println(s"[parity-jvm] finished: $done fixtures, valid $valid, invalid $invalid, error $error")
  }
}
