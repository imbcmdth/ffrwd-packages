#!/usr/bin/env bash
# Every function compiles, and every function reaches the command the
# longhand spelling reaches. Each `<name>.sql` calls core; the `.longhand.sql`
# beside it writes the same job out by hand, and the two commands have to
# match byte for byte. That is the whole claim these functions make: a name
# for a spelling, not a second way through the compiler.
#
# The fixture is five seconds of lavfi with two tagged audio tracks, made here
# and not committed. FFRWD and FFMPEG override the commands for a checkout.
set -u
cd "$(dirname "$0")/.." || exit 1

FFRWD=${FFRWD:-ffrwd}
FFMPEG=${FFMPEG:-ffmpeg}
FIXTURE=tests/fixtures/av.mp4

# The queries call ffrwd.core.* by name, and a link is how a working
# directory answers to its name. Re-running it re-points at this checkout,
# which is what a test of this checkout wants.
$FFRWD link >/dev/null || { echo "could not link this package"; exit 1; }

if [ ! -f "$FIXTURE" ]; then
  mkdir -p tests/fixtures
  $FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=640x360:rate=30:duration=5" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=5" \
    -f lavfi -i "sine=frequency=660:sample_rate=48000:duration=5" \
    -map 0:v -map 1:a -map 2:a -c:v libx264 -pix_fmt yuv420p -c:a aac \
    -metadata:s:a:0 language=eng -metadata:s:a:1 language=fra \
    "$FIXTURE" || { echo "could not make $FIXTURE"; exit 1; }
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pass=0
fail=0

for q in tests/*.sql; do
  case "$q" in *.longhand.sql) continue ;; esac
  name=$(basename "$q" .sql)
  hand="tests/$name.longhand.sql"

  if ! $FFRWD compile -f "$q" >"$work/$name.out" 2>&1; then
    echo "FAIL $name: does not compile"
    sed 's/^/      /' "$work/$name.out"
    fail=$((fail + 1))
    continue
  fi

  if [ ! -f "$hand" ]; then
    echo "pass $name (compiles)"
    pass=$((pass + 1))
    continue
  fi

  if ! $FFRWD compile -f "$hand" >"$work/$name.hand" 2>&1; then
    echo "FAIL $name: the longhand does not compile"
    sed 's/^/      /' "$work/$name.hand"
    fail=$((fail + 1))
    continue
  fi

  if cmp -s "$work/$name.out" "$work/$name.hand"; then
    echo "pass $name (compiles, matches the longhand)"
    pass=$((pass + 1))
  else
    echo "FAIL $name: the command differs from the longhand"
    diff "$work/$name.hand" "$work/$name.out" | sed 's/^/      /'
    fail=$((fail + 1))
  fi
done

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
