#!/usr/bin/env bash
# Benchmark limits against plain C, Python 3 and the v0.1 tree-walking interpreter.
# Usage: bench/run.sh [timeout_seconds]   (run from anywhere)
# Reports best-of-3 wall-clock run time (build time excluded) in seconds.
set -u
cd "$(dirname "$0")"
TO=${1:-120}
ROOT=$(cd ../.. && pwd)
LIMC=../target/release/limits
LIM=$ROOT/compiler/target/release/lim
OUT=${TMPDIR:-/tmp}/limits-bench-$$
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT
(cd .. && cargo build --release -q) || exit 1
(cd "$ROOT/compiler" && cargo build --release -q) || exit 1

# Compare the last run's stdout with the limits checked output (floats may
# differ in the last digits between Python and C formatting; shown, not hidden).
same() { if cmp -s "$OUT/stdout" "$OUT/want"; then :; else echo "  note: $1 output differs: $(tr '\n' ' ' < "$OUT/stdout")" >> "$OUT/notes"; fi; }

# best-of-3 wall time of "$@" in seconds, or "timeout"/"error"
best() {
  local best="" t0 t1 t
  for _ in 1 2 3; do
    t0=$(date +%s%N)
    if ! timeout "$TO" "$@" >"$OUT/stdout" 2>"$OUT/stderr"; then
      [ $? -eq 124 ] && { echo "timeout(>${TO}s)"; return; }
      echo "error"; return
    fi
    t1=$(date +%s%N)
    t=$(( (t1 - t0) / 1000000 ))
    if [ -z "$best" ] || [ "$t" -lt "$best" ]; then best=$t; fi
  done
  printf '%d.%03d' $((best / 1000)) $((best % 1000))
}

printf '%-8s %-10s %-10s %-10s %-10s %-10s %-16s\n' bench limits limits-unchk limits-O0 C python3 lim-interp
for b in fib loops sieve nbody strings; do
  $LIMC build $b.lim -o "$OUT/$b" --max-steps 100000000000 || exit 1
  $LIMC build $b.lim -o "$OUT/$b.u" --unchecked || exit 1
  $LIMC build $b.lim -o "$OUT/$b.d" --debug --max-steps 100000000000 || exit 1
  r1=$(best "$OUT/$b"); cp "$OUT/stdout" "$OUT/want"
  r2=$(best "$OUT/$b.u"); same "$b unchecked"
  r0=$(best "$OUT/$b.d"); same "$b -O0"
  r3=-
  if [ -f $b.c ]; then cc -O2 -o "$OUT/$b.c.bin" $b.c && r3=$(best "$OUT/$b.c.bin") && same "$b C"; fi
  r4=$(best python3 $b.py); same "$b python3"
  r5=-
  if [ -f $b.dyn.lim ]; then r5=$(best "$LIM" run $b.dyn.lim --max-steps 100000000000 --max-depth 100000) && same "$b lim-interp"; fi
  printf '%-8s %-10s %-10s %-10s %-10s %-10s %-16s\n' $b "$r1" "$r2" "$r0" "$r3" "$r4" "$r5"
  echo "  limits output: $(tr '\n' ' ' < "$OUT/want")" >> "$OUT/notes"
done
cat "$OUT/notes"

echo
echo "compile time (limits build --time, one run each):"
for b in fib loops sieve nbody strings; do
  for mode in --release --debug; do
    printf '%-8s %-10s ' $b $mode
    $LIMC build $b.lim -o "$OUT/x" $mode --time 2>&1 >/dev/null | tail -1
  done
done

echo
echo "synthetic large program (gen_large.py 1000):"
python3 gen_large.py 1000 > "$OUT/large.lim"
echo "  $(wc -l < "$OUT/large.lim") lines"
printf '  check     '; $LIMC check "$OUT/large.lim" --time 2>&1 | tail -1
for mode in --release --debug; do
  printf '  %-9s ' $mode; $LIMC build "$OUT/large.lim" -o "$OUT/large" $mode --time 2>&1 | tail -1
done
