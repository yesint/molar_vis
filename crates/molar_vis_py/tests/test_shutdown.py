"""GPU subprocess regressions; run with MOLAR_VIS_TEST_GPU=1 and the extension on PYTHONPATH."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
PROBE = r'''
import atexit
import os
from pathlib import Path
import sys
import time

viewer_tid = None

# Register first so this runs AFTER the extension's shutdown handler. Checking
# native threads catches the lifecycle bug even on drivers that do not segfault.
def audit():
    try:
        if viewer_tid is not None and Path(f'/proc/self/task/{viewer_tid}').exists():
            print('viewer thread survived interpreter shutdown', file=sys.stderr, flush=True)
            os._exit(7)
        print('viewers joined', flush=True)
    except BaseException:
        os._exit(8)

atexit.register(audit)
import molar_vis as mv
mode = sys.argv[1]
if mode != 'import':
    viewer = mv.spawn(visible=False)
    deadline = time.monotonic() + 10
    while viewer_tid is None:
        candidates = []
        for path in Path('/proc/self/task').glob('*/comm'):
            try:
                if path.read_text().strip() == 'molar_vis-ui':
                    candidates.append(int(path.parent.name))
            except FileNotFoundError:
                pass
        # This isolated process creates one viewer. Its later Rayon workers can
        # inherit the OS name, so retain the original viewer's TID, not its name.
        if candidates:
            viewer_tid = min(candidates)
        else:
            assert time.monotonic() < deadline, 'viewer thread did not start'
            time.sleep(0.001)
    if mode in ('render', 'explicit'):
        system = mv.System('tests/2lao.pdb')
        molecule = viewer.add_mol(system)
        molecule.add_rep(system('protein'), style='cartoon')
        viewer.render(sys.argv[2], width=160, height=120)
    if mode == 'explicit':
        mv._shutdown_viewers()
        mv._shutdown_viewers()
        try:
            mv.spawn(visible=False)
        except RuntimeError:
            pass
        else:
            raise AssertionError('spawn accepted after shutdown')
'''


@unittest.skipUnless(
    sys.platform.startswith('linux') and os.environ.get('MOLAR_VIS_TEST_GPU') == '1',
    'requires Linux /proc, a native GPU/display, and MOLAR_VIS_TEST_GPU=1',
)
class ViewerShutdown(unittest.TestCase):
    def probe(self, mode, cold=False):
        with tempfile.TemporaryDirectory(prefix='molar-viewer-exit-') as directory:
            env = {k: v for k, v in os.environ.items() if not k.startswith('MOLAR_VIS_DEBUG_')}
            env['MOLAR_VIS_DEBUG_DEFAULTS'] = '1'
            if cold:
                env['MESA_SHADER_CACHE_DIR'] = str(Path(directory) / 'shader-cache')
            result = subprocess.run(
                [sys.executable, '-c', PROBE, mode, str(Path(directory) / 'scene.png')],
                cwd=ROOT, env=env, capture_output=True, text=True, timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn('viewers joined', result.stdout)
            if mode in ('render', 'explicit'):
                self.assertTrue((Path(directory) / 'scene.png').is_file())

    def test_import_without_viewer(self):
        self.probe('import')

    def test_exit_during_startup(self):
        self.probe('immediate')

    def test_exit_during_cold_shader_compilation(self):
        self.probe('immediate', cold=True)

    def test_repeated_exit_with_rendered_shared_system(self):
        for attempt in range(6):
            with self.subTest(attempt=attempt):
                self.probe('render')

    def test_explicit_shutdown_is_idempotent_and_rejects_new_viewers(self):
        self.probe('explicit')


if __name__ == '__main__':
    unittest.main()
