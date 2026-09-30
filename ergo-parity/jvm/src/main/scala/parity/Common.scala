package parity

import com.typesafe.config.ConfigFactory
import net.ceedubs.ficus.Ficus._
import net.ceedubs.ficus.readers.ArbitraryTypeReader._
import org.ergoplatform.settings.{ChainSettings, ModifierIdReader, PowSchemeReaders, SettingsReaders}

object Hex {
  def decode(s: String): Array[Byte] = {
    require(s.length % 2 == 0, s"odd hex length ${s.length}")
    val out = new Array[Byte](s.length / 2)
    var i = 0
    while (i < out.length) {
      val hi = Character.digit(s.charAt(2 * i), 16)
      val lo = Character.digit(s.charAt(2 * i + 1), 16)
      require(hi >= 0 && lo >= 0, s"bad hex at $i")
      out(i) = ((hi << 4) | lo).toByte
      i += 1
    }
    out
  }

  def encode(b: Array[Byte]): String = {
    val sb = new java.lang.StringBuilder(b.length * 2)
    b.foreach(x => { sb.append(Character.forDigit((x >> 4) & 0xf, 16)); sb.append(Character.forDigit(x & 0xf, 16)) })
    sb.toString
  }
}

/** Mainnet chain settings, read with the node's own reader classes from the node's own mainnet.conf. */
object Chain extends PowSchemeReaders with ModifierIdReader with SettingsReaders {
  lazy val mainnet: ChainSettings = {
    val cfg = ConfigFactory.parseResources("mainnet.conf")
      .withFallback(ConfigFactory.parseResources("ergo-application.conf"))
      .withFallback(ConfigFactory.systemProperties())
      .resolve()
    cfg.as[ChainSettings]("ergo.chain")
  }
}
