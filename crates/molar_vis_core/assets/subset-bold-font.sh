#!/bin/sh
# Regenerate the embedded bold face. Run from the repo root; needs fonttools
# (`pip install --user fonttools`). See the note in theme.rs.
#
# DejaVu Sans Bold ships ~6000 glyphs (692 kB) and we need Latin plus a handful of
# typographic/scientific characters, so it is subset to ~30 kB before being embedded.
set -e
SRC=/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf
OUT=crates/molar_vis_core/assets/DejaVuSans-Bold-subset.ttf
pyftsubset "$SRC" \
    --output-file="$OUT" \
    --unicodes=0020-00FF \
    --text='…–—‘’“”•°±×÷≈≤≥≠²³⁻μÅαβγδπΣΩ→←↑↓⇄✓⚠' \
    --layout-features='' \
    --no-hinting \
    --desubroutinize \
    --drop-tables+=DSIG
ls -la "$OUT"
