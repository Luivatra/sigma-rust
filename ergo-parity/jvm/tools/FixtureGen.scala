package parity

import io.circe.{Decoder, Json, parser}
import org.ergoplatform.ErgoBox
import org.ergoplatform.modifiers.history.extension.Extension
import org.ergoplatform.modifiers.history.header.{Header, HeaderSerializer}
import org.ergoplatform.modifiers.mempool.{ErgoTransaction, ErgoTransactionSerializer}
import org.ergoplatform.wallet.boxes.ErgoBoxSerializer
import org.ergoplatform.wallet.serialization.JsonCodecsWrapper

import java.io.{BufferedWriter, FileWriter}
import java.net.URI
import java.net.http.{HttpClient, HttpRequest, HttpResponse}
import scala.collection.mutable

/**
  * Independent fixture producer for the JVM runner (cross-check of the Rust extractor).
  *
  * The JSON -> bytes step uses the node's own JSON decoders and serializers from ergo-core, so the bytes are what
  * the node itself would serialize. Every rebuilt object is checked against its id (header id, tx id, box id).
  *
  * usage: FixtureGen --node http://host:9053 --from 700000 --to 700050 --out fixtures.jsonl
  */
object FixtureGen {
  private val http = HttpClient.newHttpClient()
  private var node = "http://127.0.0.1:9053"

  private def get(path: String): Json = {
    val req = HttpRequest.newBuilder(URI.create(node + path)).GET().build()
    val resp = http.send(req, HttpResponse.BodyHandlers.ofString())
    require(resp.statusCode() == 200, s"GET $path -> ${resp.statusCode()}: ${resp.body().take(200)}")
    parser.parse(resp.body()).fold(throw _, identity)
  }

  private def dec[T: Decoder](j: Json, what: String): T = j.as[T].fold(e => sys.error(s"$what: $e"), identity)

  private implicit val boxDecoder: Decoder[ErgoBox] = JsonCodecsWrapper.ergoBoxDecoder

  private val headerCache = mutable.HashMap.empty[Int, Header]
  private val blockCache = mutable.HashMap.empty[Int, Json]

  private def blockAt(h: Int): Json = blockCache.getOrElseUpdate(h, {
    val ids = get(s"/blocks/at/$h").as[List[String]].fold(throw _, identity)
    get(s"/blocks/${ids.head}")
  })

  private def headerAt(h: Int): Header = headerCache.getOrElseUpdate(h, {
    val jh = blockAt(h).hcursor.downField("header").focus.get
    val hdr = dec[Header](jh, s"header $h")
    val want = jh.hcursor.downField("id").as[String].fold(throw _, identity)
    require(hdr.encodedId == want, s"header id mismatch at $h: ${hdr.encodedId} != $want")
    hdr
  })

  def main(args: Array[String]): Unit = {
    var from, to = -1
    var out = ""
    var i = 0
    while (i < args.length) {
      args(i) match {
        case "--node" => node = args(i + 1)
        case "--from" => from = args(i + 1).toInt
        case "--to" => to = args(i + 1).toInt
        case "--out" => out = args(i + 1)
        case o => sys.error(s"unknown argument $o")
      }
      i += 2
    }
    require(from >= 0 && to >= from && out.nonEmpty, "usage: FixtureGen --node URL --from H --to H --out FILE")
    val w = new BufferedWriter(new FileWriter(out))
    var n = 0
    try {
      for (h <- from to to) {
        val block = blockAt(h)
        val header = headerAt(h)
        val txsJson = block.hcursor.downField("blockTransactions").downField("transactions").as[List[Json]].fold(throw _, identity)
        val txs = txsJson.map(j => dec[ErgoTransaction](j, s"tx at $h"))
        txs.zip(txsJson).foreach { case (tx, j) =>
          val want = j.hcursor.downField("id").as[String].fold(throw _, identity)
          require(tx.id == want, s"tx id mismatch at $h: ${tx.id} != $want")
        }
        val created = txs.flatMap(_.outputs).map(b => Hex.encode(b.id) -> b).toMap
        val boxCache = mutable.HashMap.empty[String, ErgoBox]
        def box(id: Array[Byte]): ErgoBox = {
          val hid = Hex.encode(id)
          boxCache.getOrElseUpdate(hid, created.getOrElse(hid, {
            val b = dec[ErgoBox](get(s"/blockchain/box/byId/$hid"), s"box $hid")
            require(Hex.encode(b.id) == hid, s"box id mismatch: ${Hex.encode(b.id)} != $hid")
            b
          }))
        }

        val epochStart = h - h % 1024
        val extJson = blockAt(epochStart).hcursor.downField("extension").focus.get
        val ext = dec[Extension](extJson, "extension")
        val fields = ext.fields
        val params = fields.filter(f => f._1(0) == 0 && f._1(1) != 124 && f._2.length == 4).map { case (k, v) =>
          k(1).toString -> Json.fromInt(java.nio.ByteBuffer.wrap(v).getInt)
        }
        val prevHeaders = (1 to 10).filter(h - _ >= 0).map(d => headerAt(h - d))

        txs.zipWithIndex.foreach { case (tx, idx) =>
          val fixture = Json.obj(
            "schema" -> Json.fromInt(1),
            "height" -> Json.fromInt(h),
            "block_id" -> Json.fromString(header.encodedId),
            "tx_index" -> Json.fromInt(idx),
            "tx_id" -> Json.fromString(tx.id),
            "tx" -> Json.fromString(Hex.encode(ErgoTransactionSerializer.toBytes(tx))),
            "inputs" -> Json.arr(tx.inputs.map(in => Json.fromString(Hex.encode(ErgoBoxSerializer.toBytes(box(in.boxId))))): _*),
            "data_inputs" -> Json.arr(tx.dataInputs.map(in => Json.fromString(Hex.encode(ErgoBoxSerializer.toBytes(box(in.boxId))))): _*),
            "headers" -> Json.arr(prevHeaders.map(p => Json.fromString(Hex.encode(HeaderSerializer.toBytes(p)))): _*),
            "header" -> Json.fromString(Hex.encode(HeaderSerializer.toBytes(header))),
            "parameters" -> Json.fromFields(params),
            "epoch_extension" -> Json.arr(fields.map { case (k, v) =>
              Json.arr(Json.fromString(Hex.encode(k)), Json.fromString(Hex.encode(v))) }: _*),
            "notes" -> Json.arr(Json.fromString("reencoded"), Json.fromString("jvm-fixturegen")))
          w.write(fixture.noSpaces); w.write('\n')
          n += 1
        }
        System.err.println(s"[fixturegen] height $h: ${txs.size} txs")
      }
    } finally w.close()
    System.err.println(s"[fixturegen] wrote $n fixtures to $out")
  }
}
