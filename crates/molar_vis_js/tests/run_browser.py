"""Exercise the real wasm embedding API in an isolated Chromium profile."""
import argparse
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import os
import signal
import subprocess
import tempfile
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", required=True, type=Path)
    parser.add_argument("--browser", default="chromium-browser")
    args = parser.parse_args()
    package = args.package.resolve()
    done = threading.Event()
    reports = []

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(Path(__file__).parent), **kw)

        def translate_path(self, path):
            if path in {"/pkg/molar_vis.js", "/pkg/molar_vis_bg.wasm"}:
                return str(package / path.rsplit("/", 1)[1])
            return super().translate_path(path)

        def log_message(self, *args):
            pass

        def do_POST(self):
            reports.append(self.rfile.read(min(int(self.headers.get("Content-Length", "0")), 10000)).decode())
            self.send_response(204)
            self.end_headers()
            if reports[-1].startswith(("PASS:", "FAIL:")):
                done.set()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="molar-vis-embedding-") as directory:
            with (Path(directory) / "browser.log").open("w+") as log:
                process = subprocess.Popen([
                    args.browser, "--headless", "--enable-logging=stderr", "--no-first-run", "--disable-background-networking",
                    "--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
                    f"--user-data-dir={directory}/profile", f"http://127.0.0.1:{server.server_port}/embedding.html",
                ], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    if not done.wait(90) or not reports[-1].startswith("PASS:"):
                        log.seek(0)
                        raise RuntimeError(str(reports) + "\n" + log.read()[-5000:])
                    print(reports[-1])
                finally:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    main()
