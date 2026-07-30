#!/bin/sh
# Local verification pass: tests, a wasm build check, and the headless hooks.
# Warm-cache, run by hand — deliberately NOT a CI job. A cold build of this dep
# tree (591 packages incl. tract + wgpu, and 6 git deps with no crates.io cache
# reuse) is tens of minutes; CI here is release-tag-only by design.
#
# Usage:
#   sh scripts/check.sh            run everything; diff against the baseline if one exists
#   sh scripts/check.sh --record   run everything, then (re)record that output as the baseline
#
# Output goes to target/check, the baseline to target/check-baseline (both
# gitignored via /target). Override with OUT= / BASE=.
#
# Pure refactors should come back byte-identical: same session JSON, same pixels.
# A CHANGED line is either a real regression or an intended visual change you
# should re-record deliberately.

set -e

cd "$(dirname "$0")/.."

OUT=${OUT:-target/check}
BASE=${BASE:-target/check-baseline}
ARTIFACTS="vdw.png licorice.png cartoon.png surface.png session_a.json"

RECORD=0
[ "$1" = "--record" ] && RECORD=1

rm -rf "$OUT"
mkdir -p "$OUT"

echo "== tests (default features) =="
cargo test -q -p molar_vis_core
# molar_vis_py is excluded on purpose: it enables pyo3 `extension-module`
# unconditionally and is cdylib-only, so a test harness for it finds no libpython.

echo "== tests (--features scripting) =="
cargo test -q -p molar_vis_core --features scripting

echo "== wasm32 build check =="
cargo build -q --target wasm32-unknown-unknown -p molar_vis_core

# _HIDDEN keeps the window off the desktop (this box is shared), _EXIT self-quits
# at the end of App::new once the file-producing hooks have run, and _DEFAULTS
# skips the config file so runs are reproducible and never touch a saved config.
export MOLAR_VIS_DEBUG_HIDDEN=1
export MOLAR_VIS_DEBUG_EXIT=1
export MOLAR_VIS_DEBUG_DEFAULTS=1

echo "== session save -> load -> save is byte-identical =="
MOLAR_VIS_DEBUG_SAVE_SESSION="$OUT/session_a.json" \
	cargo run -q -p molar_vis -- tests/2lao.pdb
MOLAR_VIS_DEBUG_LOAD_SESSION="$OUT/session_a.json" \
MOLAR_VIS_DEBUG_SAVE_SESSION="$OUT/session_b.json" \
	cargo run -q -p molar_vis -- tests/2lao.pdb
cmp "$OUT/session_a.json" "$OUT/session_b.json"
echo "  ok"

echo "== renders =="
for r in vdw licorice cartoon surface; do
	MOLAR_VIS_DEBUG_REP=$r MOLAR_VIS_DEBUG_SAVE_IMAGE="$OUT/$r.png" \
		cargo run -q -p molar_vis -- tests/2lao.pdb
	echo "  $r -> $OUT/$r.png"
done

if [ "$RECORD" = 1 ]; then
	rm -rf "$BASE"
	cp -r "$OUT" "$BASE"
	echo "== baseline recorded in $BASE =="
	exit 0
fi

if [ -d "$BASE" ]; then
	echo "== diff vs baseline ($BASE) =="
	changed=0
	for f in $ARTIFACTS; do
		if cmp -s "$OUT/$f" "$BASE/$f"; then
			echo "  same     $f"
		else
			echo "  CHANGED  $f"
			changed=1
		fi
	done
	if [ "$changed" != 0 ]; then
		echo "!! output differs from the baseline (see above)"
		exit 1
	fi
else
	echo "== no baseline yet; record one with: sh scripts/check.sh --record =="
fi

echo "== ok =="
