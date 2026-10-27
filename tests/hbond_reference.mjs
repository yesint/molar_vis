// Rebuild the frozen geometry expectations with the actual pinned Mol* function:
// node -e "import('./tests/hbond_reference.mjs').then(m => m.generate())"
// Mol* is MIT licensed; see crates/molar_vis_core/licenses/molstar.txt.
export const revision = 'fe1ac1555001d1acb023077cd4dc6ca1d06870f8';

export function evaluate(source, cases) {
    // Extract the function body without changing its predicates. The adapters
    // below expose each small test molecule through Mol*'s geometry interface.
    const start = source.indexOf('function checkGeometryOf(');
    const bodyStart = source.indexOf('  const donIndex', start);
    const bodyEnd = source.indexOf('\nfunction testHydrogenBond(', bodyStart);
    const body = source.slice(bodyStart, bodyEnd).trim().replace(/\}$/, '');
    const sub = (a, b) => a.map((v, i) => v - b[i]);
    const dot = (a, b) => a.reduce((n, v, i) => n + v * b[i], 0);
    const angle = (a, b) => Math.acos(Math.max(-1, Math.min(1, dot(a, b) / Math.sqrt(dot(a, a) * dot(b, b)))));
    const geometry = { Linear: 2, Trigonal: 3, Tetrahedral: 4 };
    const ideal = new Map([[2, Math.PI], [3, Math.PI * 2 / 3], [4, 109.4721 * Math.PI / 180]]);
    const angles = (_, unit, index, other, oi, ignoreH, heavy, hs) => {
        heavy.length = hs.length = 0;
        const origin = unit.atoms[index];
        const direction = sub(other.atoms[oi], origin);
        for (const p of unit.heavy) heavy.push(angle(direction, sub(p, origin)));
        if (!ignoreH) for (const p of unit.h) hs.push(angle(direction, sub(p, origin)));
    };
    const plane = (_, unit, index, other, oi) => unit.normal
        ? Math.abs(Math.PI / 2 - angle(unit.normal, sub(other.atoms[oi], unit.atoms[index]))) : undefined;
    const closest = (_, unit, index, other, oi, out) => {
        out.unit = unit; out.index = index;
        let min = dot(sub(unit.atoms[index], other.atoms[oi]), sub(unit.atoms[index], other.atoms[oi]));
        unit.h.forEach((p, i) => {
            const delta = sub(p, other.atoms[oi]);
            if (dot(delta, delta) < min) { min = dot(delta, delta); out.index = i + 1; }
        });
    };
    const fn = new Function('AtomGeometryAngles', 'AtomGeometry', 'calcAnglesInto', 'calcPlaneAngle',
        'closestHydrogenInto', `const DonAngles=[], DonHAngles=[], AccAngles=[], AccHAngles=[], Donor={};
        const deg120InRad=Math.PI*2/3;
        return function(structure, don, acc, opts) { ${body} };`)(ideal, geometry, angles, plane, closest);
    const feature = site => ({ members: [0], offsets: [0], feature: 0,
        idealGeometry: [geometry[site.geometry]],
        unit: { ...site, atoms: [site.pos, ...site.h] } });
    const opts = { includeBackbone: true, ignoreHydrogens: false,
        maxDonAngleDev: Math.PI / 4, maxAccAngleDev: Math.PI / 4,
        maxDonOutOfPlaneAngle: Math.PI / 4, maxAccOutOfPlaneAngle: Math.PI / 2 };
    return cases.map(c => ({ ...c, accepted: fn(null, feature(c.donor), feature(c.acceptor), opts) === true }));
}

export async function generate() {
    const fs = await import('node:fs/promises');
    const url = `https://raw.githubusercontent.com/molstar/molstar/${revision}/packages/model/src/props/computed/interactions/hydrogen-bonds.ts`;
    const source = await (await fetch(url)).text();
    const path = new URL('./hbond_geometry_reference.json', import.meta.url);
    const cases = JSON.parse(await fs.readFile(path, 'utf8'));
    await fs.writeFile(path, JSON.stringify(evaluate(source, cases), null, 2) + '\n');
}
