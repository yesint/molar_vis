#!/bin/sh
# Regenerate the embedded bold face from a full Ubuntu-Bold.ttf.
#
# Ubuntu **Bold** is the bold sibling of egui's bundled Ubuntu-Light base font (same family,
# same designer), so bold text matches the rest of the UI instead of reading as a second
# typeface. Licence: Ubuntu Font Licence 1.0 (see Ubuntu-Bold-UFL.txt) — redistributable.
#
# The stock face is 324 kB for ~1200 glyphs; the UI needs Latin-1 plus a handful of
# typographic/scientific characters, so it is subset before being embedded.
#
# Usage:  sh crates/molar_vis_core/assets/subset-bold-font.sh path/to/Ubuntu-Bold.ttf
# Needs:  pip install fonttools     (build-time only — nothing ships with the app but the .ttf)
# Source: https://github.com/google/fonts/tree/main/ufl/ubuntu
set -e
SRC="${1:?usage: $0 path/to/Ubuntu-Bold.ttf}"
OUT="$(dirname "$0")/Ubuntu-Bold-subset.ttf"
pyftsubset "$SRC" \
    --output-file="$OUT" \
    --unicodes=0020-00FF \
    --text='…–—‘’“”•°±×÷≈≤≥≠²³⁻μÅαβγδπΣΩ→←↑↓⇄✓⚠' \
    --layout-features='' \
    --no-hinting \
    --desubroutinize \
    --drop-tables+=DSIG
ls -la "$OUT"
