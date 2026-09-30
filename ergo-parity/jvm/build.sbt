// JVM reference runner for ergo-parity. Uses the node's own validation code:
// ergo-core 6.1.2 (built from ergo tag v6.1.2, see README) on sigma-state 6.0.2.
name := "ergo-parity-jvm"
version := "0.1.0"
scalaVersion := "2.12.20"

resolvers += Resolver.mavenLocal
resolvers += "Sonatype Releases" at "https://oss.sonatype.org/content/repositories/releases/"
resolvers += "Sonatype Public" at "https://oss.sonatype.org/content/groups/public"
resolvers += "Repo for leveldbjni-all" at "https://gitlab.com/api/v4/projects/61211221/packages/maven"

libraryDependencies ++= Seq(
  ("org.scorexfoundation" %% "sigma-state" % "6.0.2")
    .exclude("ch.qos.logback", "logback-classic")
    .exclude("org.scorexfoundation", "scrypto"),
  "org.ergoplatform" %% "ergo-core" % "6.1.2",
  "ch.qos.logback" % "logback-classic" % "1.3.5"
)

// also compile the fixture generator kept under tools/
Compile / unmanagedSourceDirectories += baseDirectory.value / "tools"

Compile / mainClass := Some("parity.Runner")
scalacOptions ++= Seq("-deprecation", "-feature")
