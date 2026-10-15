"""With ELEMENTS_DB_VIA set and a remote ELEMENTS_DB, run this script again
through db/via.sh, which tunnels the database via the VM (see db/tunnel.sh)."""

import os
import sys
import urllib.parse


def ensure():
    db = os.environ.get("ELEMENTS_DB", "")
    host = urllib.parse.urlparse(db).hostname or ""
    if os.environ.get("ELEMENTS_DB_VIA") and host not in ("", "127.0.0.1", "localhost", "::1"):
        via = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "via.sh")
        os.execv(via, [via, sys.executable, *sys.argv])


def psql(db, *args, input=None):
    """psql's stdout; on failure exit with its stderr only (the command holds
    the connection string, which a CalledProcessError would print)."""
    import subprocess
    r = subprocess.run(["psql", db, *args], input=input, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"psql failed: {r.stderr.strip()}")
    return r.stdout
