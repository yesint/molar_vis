#!/bin/sh
# Regenerate the embedded Ubuntu faces from the full TTFs.
#
# egui bundles only **Ubuntu-Light**, which reads too thin for this UI, and no bold face at
# all. Both are replaced/added from the same family — Regular (400) as the base face and Bold
# (700) for emphasis — so weights match each other and nothing looks like a second typeface.
# Licence: Ubuntu Font Licence 1.0 (see Ubuntu-UFL.txt) — redistributable.
#
# The stock faces are ~344 kB each for ~1200 glyphs; the UI needs Latin-1 plus a handful of
# typographic/scientific characters, so both are subset to ~18 kB before being embedded.
#
# Usage:  sh crates/molar_vis_core/assets/subset-fonts.sh <dir-with-Ubuntu-Regular.ttf-and-Bold>
# Needs:  pip install fonttools     (build-time only — nothing ships but the .ttf files)
# Source: https://github.com/google/fonts/tree/main/ufl/ubuntu
set -e
SRC="${1:?usage: $0 <dir containing Ubuntu-Regular.ttf and Ubuntu-Bold.ttf>}"
OUT="$(dirname "$0")"
for face in Regular Bold; do
    pyftsubset "$SRC/Ubuntu-$face.ttf" \
        --output-file="$OUT/Ubuntu-$face-subset.ttf" \
        --unicodes=0020-00FF \
        --text='…–—‘’“”•°±×÷≈≤≥≠²³⁻μÅαβγδπΣΩ→←↑↓⇄✓⚠' \
        --layout-features='' \
        --no-hinting \
        --desubroutinize \
        --drop-tables+=DSIG
done
ls -la "$OUT"/Ubuntu-*-subset.ttf
