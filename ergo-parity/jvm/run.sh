#!/usr/bin/env bash
# JVM reference runner for ergo-parity.
#   ./run.sh --in fixtures.jsonl --out jvm.jsonl [--threads N]
# Builds on first use (and when sources change). Needs a JDK 17 and sbt; both are looked up below.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export JAVA_HOME="${PARITY_JAVA_HOME:-/usr/lib/jvm/java-17-openjdk}"
export PATH="$JAVA_HOME/bin:$PATH"
main="${PARITY_MAIN:-parity.Runner}"

cp_file="$here/target/classpath.txt"
stamp="$here/target/.built"
if [ ! -f "$cp_file" ] || [ ! -f "$stamp" ] || \
   [ -n "$(find "$here/src" "$here/tools" "$here/build.sbt" -newer "$stamp" -type f 2>/dev/null | head -1)" ]; then
  echo "[run.sh] building (JDK: $JAVA_HOME)" >&2
  mkdir -p "$here/target"
  if ! (cd "$here" && sbt -batch compile "export Runtime/fullClasspath" > target/.sbt.log 2>&1); then
    tail -n 40 "$here/target/.sbt.log" >&2; exit 1
  fi
  tail -n 1 "$here/target/.sbt.log" > "$cp_file"
  touch "$stamp"
fi
exec java -Xss64m -Xmx${PARITY_XMX:-4g} -XX:+IgnoreUnrecognizedVMOptions \
  -cp "$(cat "$cp_file")" "$main" "$@"
