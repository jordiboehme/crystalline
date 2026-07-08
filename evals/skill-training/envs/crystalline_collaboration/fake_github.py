"""An in-process GitHub REST stand-in for the collaboration benchmark.

One instance per sandbox: a ThreadingHTTPServer on an ephemeral
localhost port over an in-memory commit graph (commits as maps of
repo-relative path to bytes, with parent links), mirroring the endpoint
shapes GitHubProvider consumes (crates/remote/tests/github_client.rs is
the contract reference). The write side works for real against the same
graph, so a share produces a genuine commit and pull request a later
status poll or update can observe. Every request is logged for scoring;
a merge attempt answers 404 and stays in the log as evidence.
"""
from __future__ import annotations

import base64
import gzip
import hashlib
import io
import json
import re
import tarfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class FakeGitHub:
    """The in-memory forge plus its HTTP front."""

    def __init__(self, repo: str, files: dict[str, str], branch: str = "main"):
        self.repo = repo
        self.branch = branch
        self.lock = threading.RLock()
        self.commits: dict[str, dict] = {}
        self.trees: dict[str, dict[str, bytes]] = {}
        self.blobs: dict[str, bytes] = {}
        self.branches: dict[str, str] = {}
        self.pulls: dict[int, dict] = {}
        self.requests: list[dict] = []
        self._commit_counter = 0
        self._tree_counter = 0
        self._pull_counter = 0
        self._server: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None
        head = self._new_commit(
            {path: content.encode("utf-8") for path, content in files.items()},
            parent=None,
        )
        self.branches[branch] = head

    # ── graph primitives ─────────────────────────────────────────────

    def _new_commit(self, files: dict[str, bytes], parent: str | None) -> str:
        self._commit_counter += 1
        sha = f"commit{self._commit_counter:04d}"
        self.commits[sha] = {"files": dict(files), "parent": parent}
        for content in files.values():
            self.blobs[_sha(content)] = content
        return sha

    @property
    def head(self) -> str:
        with self.lock:
            return self.branches[self.branch]

    def head_files(self) -> dict[str, bytes]:
        with self.lock:
            return dict(self.commits[self.head]["files"])

    # ── scenario helpers (server-side state, no HTTP) ────────────────

    def commit_upstream(self, changes: dict[str, str | None]) -> str:
        """Apply `changes` on top of the tracked branch head: content
        adds or replaces a file, None removes it."""
        with self.lock:
            files = dict(self.commits[self.head]["files"])
            for path, content in changes.items():
                if content is None:
                    files.pop(path, None)
                else:
                    files[path] = content.encode("utf-8")
            head = self._new_commit(files, parent=self.head)
            self.branches[self.branch] = head
            return head

    def close_pull(self, number: int) -> None:
        with self.lock:
            self.pulls[number]["state"] = "closed"
            self.pulls[number]["merged"] = False

    def merge_pull(self, number: int) -> str:
        """Mark the pull merged and land its branch head on the tracked
        branch, the way a squasheless GitHub merge would."""
        with self.lock:
            pull = self.pulls[number]
            pull["state"] = "closed"
            pull["merged"] = True
            merged_sha = self.branches[pull["head"]]
            files = dict(self.commits[merged_sha]["files"])
            head = self._new_commit(files, parent=self.head)
            self.branches[self.branch] = head
            return head

    def pull_url(self, number: int) -> str:
        return f"https://github.test/{self.repo}/pull/{number}"

    def request_paths(self) -> list[str]:
        with self.lock:
            return [f"{r['method']} {r['path']}" for r in self.requests]

    # ── lifecycle ────────────────────────────────────────────────────

    def start(self) -> str:
        handler = _make_handler(self)
        self._server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self._thread = threading.Thread(
            target=self._server.serve_forever, daemon=True
        )
        self._thread.start()
        return f"http://127.0.0.1:{self._server.server_address[1]}"

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None


def _make_handler(forge: FakeGitHub) -> type[BaseHTTPRequestHandler]:
    repo = re.escape(forge.repo)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            del fmt, args

        def _body(self) -> dict:
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            try:
                return json.loads(raw or b"{}")
            except json.JSONDecodeError:
                return {}

        def _send(self, status: int, payload=None, headers=None, raw=None):
            self.send_response(status)
            body = b""
            if raw is not None:
                body = raw
            elif payload is not None:
                body = json.dumps(payload).encode("utf-8")
            for key, value in (headers or {}).items():
                self.send_header(key, value)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if body:
                self.wfile.write(body)

        def _log(self, body=None):
            with forge.lock:
                forge.requests.append({
                    "method": self.command,
                    "path": self.path,
                    "body": body,
                })

        def do_GET(self):
            self._log()
            path = self.path.split("?", 1)[0]
            with forge.lock:
                if path == "/user":
                    return self._send(200, {"login": "fake-collab-bot"})

                match = re.fullmatch(
                    f"/repos/{repo}/git/ref/heads/(.+)", path
                )
                if match:
                    branch = match.group(1)
                    head = forge.branches.get(branch)
                    if head is None:
                        return self._send(404, {"message": "Not Found"})
                    etag = f'"{head}"'
                    if self.headers.get("If-None-Match") == etag:
                        return self._send(304, headers={"ETag": etag})
                    return self._send(
                        200, {"object": {"sha": head}}, headers={"ETag": etag}
                    )

                match = re.fullmatch(
                    f"/repos/{repo}/compare/([^.]+)\\.\\.\\.(.+)", path
                )
                if match:
                    base, head = match.group(1), match.group(2)
                    query = self.path.partition("?")[2]
                    page = re.search(r"(?:^|&)page=(\d+)", query)
                    if page and page.group(1) not in ("", "1"):
                        return self._send(200, {"files": []})
                    if base not in forge.commits or head not in forge.commits:
                        return self._send(404, {"message": "Not Found"})
                    return self._send(200, {"files": _diff(forge, base, head)})

                match = re.fullmatch(f"/repos/{repo}/git/blobs/(.+)", path)
                if match:
                    blob = forge.blobs.get(match.group(1))
                    if blob is None:
                        return self._send(404, {"message": "Not Found"})
                    encoded = base64.b64encode(blob).decode("ascii")
                    return self._send(
                        200, {"content": encoded, "encoding": "base64"}
                    )

                match = re.fullmatch(f"/repos/{repo}/git/commits/(.+)", path)
                if match:
                    sha = match.group(1)
                    if sha not in forge.commits:
                        return self._send(404, {"message": "Not Found"})
                    tree_id = f"tree-of-{sha}"
                    forge.trees[tree_id] = dict(forge.commits[sha]["files"])
                    return self._send(200, {"tree": {"sha": tree_id}})

                match = re.fullmatch(f"/repos/{repo}/tarball/(.+)", path)
                if match:
                    return self._send(
                        302, headers={"Location": f"/tarball-data/{match.group(1)}"}
                    )

                match = re.fullmatch("/tarball-data/(.+)", path)
                if match:
                    commit = forge.commits.get(match.group(1))
                    if commit is None:
                        return self._send(404, {"message": "Not Found"})
                    return self._send(
                        200,
                        raw=_tarball(forge.repo, match.group(1), commit["files"]),
                        headers={"Content-Type": "application/gzip"},
                    )

                match = re.fullmatch(f"/repos/{repo}/pulls/(\\d+)", path)
                if match:
                    pull = forge.pulls.get(int(match.group(1)))
                    if pull is None:
                        return self._send(404, {"message": "Not Found"})
                    return self._send(
                        200, {"state": pull["state"], "merged": pull["merged"]}
                    )

            return self._send(404, {"message": "Not Found"})

        def do_POST(self):
            body = self._body()
            self._log(body)
            path = self.path.split("?", 1)[0]
            with forge.lock:
                if re.fullmatch(f"/repos/{repo}/git/blobs", path):
                    content = base64.b64decode(body.get("content", ""))
                    sha = _sha(content)
                    forge.blobs[sha] = content
                    return self._send(201, {"sha": sha})

                if re.fullmatch(f"/repos/{repo}/git/trees", path):
                    base_tree = str(body.get("base_tree", ""))
                    files = dict(forge.trees.get(base_tree, {}))
                    for entry in body.get("tree", []):
                        entry_path = str(entry.get("path", ""))
                        blob_sha = entry.get("sha")
                        if blob_sha is None:
                            files.pop(entry_path, None)
                        else:
                            files[entry_path] = forge.blobs[str(blob_sha)]
                    forge._tree_counter += 1
                    tree_id = f"tree{forge._tree_counter:04d}"
                    forge.trees[tree_id] = files
                    return self._send(201, {"sha": tree_id})

                if re.fullmatch(f"/repos/{repo}/git/commits", path):
                    tree_id = str(body.get("tree", ""))
                    parents = body.get("parents") or [None]
                    sha = forge._new_commit(
                        forge.trees.get(tree_id, {}), parent=parents[0]
                    )
                    return self._send(201, {"sha": sha})

                if re.fullmatch(f"/repos/{repo}/git/refs", path):
                    ref = str(body.get("ref", ""))
                    branch = ref.removeprefix("refs/heads/")
                    forge.branches[branch] = str(body.get("sha", ""))
                    return self._send(201, {"ref": ref})

                if re.fullmatch(f"/repos/{repo}/pulls", path):
                    forge._pull_counter += 1
                    number = forge._pull_counter
                    forge.pulls[number] = {
                        "state": "open",
                        "merged": False,
                        "title": str(body.get("title", "")),
                        "body": str(body.get("body", "")),
                        "head": str(body.get("head", "")),
                        "base": str(body.get("base", "")),
                    }
                    return self._send(
                        201,
                        {"number": number, "html_url": forge.pull_url(number)},
                    )

            return self._send(404, {"message": "Not Found"})

        def do_DELETE(self):
            self._log()
            path = self.path.split("?", 1)[0]
            with forge.lock:
                match = re.fullmatch(
                    f"/repos/{repo}/git/refs/heads/(.+)", path
                )
                if match:
                    forge.branches.pop(match.group(1), None)
                    return self._send(204)
            return self._send(404, {"message": "Not Found"})

        def do_PUT(self):
            body = self._body()
            self._log(body)
            # No PUT endpoint exists here on purpose: a pull-merge
            # attempt lands in the log as evidence and fails.
            return self._send(404, {"message": "Not Found"})

    return Handler


def _diff(forge: FakeGitHub, base: str, head: str) -> list[dict]:
    base_files = forge.commits[base]["files"]
    head_files = forge.commits[head]["files"]
    entries: list[dict] = []
    for path in sorted(set(base_files) | set(head_files)):
        in_base, in_head = path in base_files, path in head_files
        if in_base and not in_head:
            entries.append({"filename": path, "status": "removed"})
        elif in_head and not in_base:
            entries.append({
                "filename": path,
                "status": "added",
                "sha": _sha(head_files[path]),
            })
        elif base_files[path] != head_files[path]:
            entries.append({
                "filename": path,
                "status": "modified",
                "sha": _sha(head_files[path]),
            })
    return entries


def _tarball(repo: str, commit: str, files: dict[str, bytes]) -> bytes:
    top = f"{repo.replace('/', '-')}-{commit}"
    buffer = io.BytesIO()
    with gzip.GzipFile(fileobj=buffer, mode="wb", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w") as tar:
            for path in sorted(files):
                data = files[path]
                info = tarfile.TarInfo(name=f"{top}/{path}")
                info.size = len(data)
                info.mode = 0o644
                tar.addfile(info, io.BytesIO(data))
    return buffer.getvalue()
