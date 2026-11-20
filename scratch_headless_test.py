#!/usr/bin/env python3
"""Headless render smoke-test for the molar_vis Python module.

Builds a protein-ligand scene entirely off-screen (no visible window), renders it
to a PNG, and verifies the file is a valid, non-blank image. Run it from the repo's
own venv:

    cd /home/semen/work/Projects/molar_vis
    source .venv/bin/activate
    python scratch_headless_test.py

The venv has only numpy (no PIL), so the PNG is decoded here with a small
stdlib-only decoder (zlib + the standard PNG unfilter).
"""

import os
import struct
import sys
import zlib

import numpy as np

import molar_vis as mv

OUT = "/tmp/mv_headless_test.png"
W, H = 1000, 800


# --------------------------------------------------------------------------- #
# Minimal PNG decoder (8-bit, non-interlaced, RGB/RGBA/gray) -> numpy array.
# --------------------------------------------------------------------------- #
def decode_png(path):
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG file")
    pos = 8
    width = height = bit_depth = color_type = None
    idat = bytearray()
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        ctype = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        pos += 12 + length  # 4 len + 4 type + data + 4 crc
        if ctype == b"IHDR":
            width, height, bit_depth, color_type = struct.unpack(">IIBB", chunk[:10])
        elif ctype == b"IDAT":
            idat += chunk
        elif ctype == b"IEND":
            break
    if width is None:
        raise ValueError("no IHDR chunk")
    if bit_depth != 8:
        raise ValueError(f"unsupported bit depth {bit_depth}")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError(f"unsupported color type {color_type}")

    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    out = np.zeros((height, stride), dtype=np.uint8)
    prev = np.zeros(stride, dtype=np.int32)
    bpp = channels  # bytes per pixel (8-bit)
    src = 0
    for y in range(height):
        ftype = raw[src]
        src += 1
        line = np.frombuffer(raw[src : src + stride], dtype=np.uint8).astype(np.int32)
        src += stride
        cur = line.copy()
        if ftype == 0:  # None
            pass
        elif ftype == 1:  # Sub
            for i in range(bpp, stride):
                cur[i] = (cur[i] + cur[i - bpp]) & 0xFF
        elif ftype == 2:  # Up
            cur = (cur + prev) & 0xFF
        elif ftype == 3:  # Average
            for i in range(stride):
                a = cur[i - bpp] if i >= bpp else 0
                b = prev[i]
                cur[i] = (cur[i] + ((a + b) >> 1)) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = cur[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                if pa <= pb and pa <= pc:
                    pr = a
                elif pb <= pc:
                    pr = b
                else:
                    pr = c
                cur[i] = (cur[i] + pr) & 0xFF
        else:
            raise ValueError(f"unknown filter type {ftype}")
        out[y] = cur.astype(np.uint8)
        prev = cur
    return out.reshape(height, width, channels)


def main():
    if os.path.exists(OUT):
        os.remove(OUT)

    # One molecule = protein, one = ligand. Load both from the repo test assets.
    # jak2.pdb is the receptor; jak2_inhs.sd holds docked ligand poses ($$$$ records)
    # in the receptor's own frame -- System(path) reads the first record.
    protein = mv.System("tests/jak2.pdb")
    ligand = mv.System("tests/jak2_inhs.sd")
    print(f"protein atoms: {len(protein('all'))}, ligand atoms: {len(ligand('all'))}")

    # Headless viewer: an invisible window, still a real wgpu device.
    v = mv.spawn(visible=False)

    m_prot = v.add_mol(protein)
    m_lig = v.add_mol(ligand)

    # Protein cartoon coloured by secondary structure.
    m_prot.add_rep(protein("protein"), style="cartoon", color="ss")

    # Residues near the ligand as Lines at width 3. The protein and ligand are
    # separate molecules, so compute the contact atoms with numpy (coords are (3, N),
    # nm) and hand them to the viewer as an explicit index selection.
    psel = protein("protein")
    pidx = np.asarray(psel.index)
    pc = np.asarray(psel.coords).T  # (Np, 3)
    lc = np.asarray(ligand("all").coords).T  # (Nl, 3)
    dmin = np.sqrt(((pc[:, None, :] - lc[None, :, :]) ** 2).sum(-1)).min(axis=1)
    near_idx = [int(i) for i in pidx[dmin < 0.45]]
    print(f"pocket atoms near ligand (<0.45 nm): {len(near_idx)}")
    if near_idx:
        near = protein(near_idx)
        m_prot.add_rep(near, style="lines", color="element", width=3.0)
    else:
        # Fallback so the lines-width path is still exercised.
        m_prot.add_rep(protein("protein and name CA"), style="lines", color="element", width=3.0)

    # Ligand as licorice.
    lig_rep = m_lig.add_rep(ligand("all"), style="licorice", color="element")

    # Protein-ligand interaction contacts (H-bonds, hydrophobic, ...).
    m_prot.add_interactions(lig_rep)

    # Zoom to the ligand, then render off-screen (blocks until the file is written).
    v.focus(m_lig, ligand("all"))
    v.reset_view()  # confirm reset_view works headlessly
    v.focus(m_lig, ligand("all"))
    v.rotate(20.0, 10.0)  # confirm rotate works headlessly
    v.zoom(1.1)  # confirm zoom works headlessly

    v.render(OUT, width=W, height=H)

    # --- Verify the PNG exists, is valid, and is not blank. ---
    if not os.path.exists(OUT):
        print("FAIL: output file was not created")
        return 1
    size = os.path.getsize(OUT)
    print(f"wrote {OUT} ({size} bytes)")

    img = decode_png(OUT)
    print(f"decoded image shape: {img.shape}, dtype {img.dtype}")
    if img.shape[0] != H or img.shape[1] != W:
        print(f"FAIL: unexpected dimensions {img.shape[:2]}, expected ({H}, {W})")
        return 1

    rgb = img[:, :, :3].astype(np.float64)
    variance = float(rgb.var())
    print(f"pixel variance: {variance:.3f}")

    # Background is a uniform colour; count pixels that differ noticeably from the
    # most common (corner) colour, i.e. actual rendered geometry.
    bg = img[0, 0, :3].astype(np.int32)
    diff = np.abs(img[:, :, :3].astype(np.int32) - bg).sum(axis=2)
    nonbg = int((diff > 24).sum())
    total = W * H
    print(f"background colour (corner): {tuple(int(x) for x in bg)}")
    print(f"non-background pixels: {nonbg} / {total} ({100.0 * nonbg / total:.2f}%)")
    print(f"distinct colours (sampled): {len(np.unique(img.reshape(-1, img.shape[2]), axis=0))}")

    ok = variance > 1.0 and nonbg > total // 1000
    print("RESULT:", "PASS - image is valid and NOT blank" if ok else "FAIL - image looks blank")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
