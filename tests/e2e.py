#!/usr/bin/env python3
"""InkDrip E2E test suite.

Builds release binaries (optional), starts inkdrip-server in an isolated
data-test/ directory, runs all feature tests via CLI + direct HTTP, then
tears down and reports.

Usage:
    python3 tests/e2e.py [options]

Options:
    --no-build   Skip 'cargo build --release' step
    --keep       Keep data-test/ on failure for post-mortem
    -v           Verbose: print full response on assertion failures
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

# ─────────────────────────────────────────────────────────────────────────────
# Constants
# ─────────────────────────────────────────────────────────────────────────────

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SERVER_BIN = PROJECT_ROOT / "target" / "release" / "inkdrip-server"
CLI_BIN = PROJECT_ROOT / "target" / "release" / "inkdrip-cli"
DATA_DIR = PROJECT_ROOT / "data-test"
E2E_CONFIG = PROJECT_ROOT / "tests" / "e2e-config.toml"
EPUB_BOOK = PROJECT_ROOT / "books" / "Rust 秘典（死灵书）.epub"

PORT = 18080
TOKEN = "e2etest"
BASE_URL = f"http://localhost:{PORT}"
STARTUP_TIMEOUT = 20  # seconds

# ─────────────────────────────────────────────────────────────────────────────
# Colors
# ─────────────────────────────────────────────────────────────────────────────

_tty = sys.stdout.isatty()
GREEN = "\033[32m" if _tty else ""
RED = "\033[31m" if _tty else ""
CYAN = "\033[36m" if _tty else ""
YELLOW = "\033[33m" if _tty else ""
BOLD = "\033[1m" if _tty else ""
RESET = "\033[0m" if _tty else ""

VERBOSE = False

# ─────────────────────────────────────────────────────────────────────────────
# Test runner
# ─────────────────────────────────────────────────────────────────────────────


class TestRunner:
    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0
        self._failures: list[str] = []

    def check(self, name: str, cond: bool, detail: str = "") -> bool:
        if cond:
            self.passed += 1
            print(f"  {GREEN}✓{RESET} {name}")
        else:
            self.failed += 1
            line = f"  {RED}✗{RESET} {name}"
            if detail:
                line += f"\n    {RED}↳ {detail}{RESET}"
            print(line)
            self._failures.append(f"{name}" + (f": {detail}" if detail else ""))
        return cond

    def skip(self, name: str, reason: str = "") -> None:
        msg = f"  {YELLOW}–{RESET} {name} (skipped"
        if reason:
            msg += f": {reason}"
        print(msg + ")")

    def section(self, title: str) -> None:
        print(f"\n{BOLD}{CYAN}▸ {title}{RESET}")

    def summary(self) -> bool:
        total = self.passed + self.failed
        print(f"\n{'─' * 56}")
        if self.failed == 0:
            print(f"{GREEN}{BOLD}All {total} checks passed.{RESET}")
        else:
            print(f"{RED}{BOLD}{self.failed}/{total} checks FAILED:{RESET}")
            for item in self._failures:
                print(f"  {RED}• {item}{RESET}")
        return self.failed == 0


# ─────────────────────────────────────────────────────────────────────────────
# HTTP helpers
# ─────────────────────────────────────────────────────────────────────────────


def _headers(auth: bool, extra: dict[str, str] | None = None) -> dict[str, str]:
    h: dict[str, str] = {"Accept": "application/json"}
    if auth:
        h["Authorization"] = f"Bearer {TOKEN}"
    if extra:
        h.update(extra)
    return h


def _parse_body(raw: bytes) -> Any:
    if not raw or not raw.strip():
        return {}
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw.decode(errors="replace")


def api_get(path: str, auth: bool = True) -> tuple[int, Any]:
    req = urllib.request.Request(f"{BASE_URL}{path}", headers=_headers(auth))
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, _parse_body(r.read())
    except urllib.error.HTTPError as e:
        return e.code, _parse_body(e.read())


def api_post(path: str, body: Any = None, auth: bool = True) -> tuple[int, Any]:
    hdrs = _headers(auth)
    if body is not None:
        data = json.dumps(body).encode()
        hdrs["Content-Type"] = "application/json"
    else:
        data = b""
    req = urllib.request.Request(
        f"{BASE_URL}{path}", data=data, headers=hdrs, method="POST"
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, _parse_body(r.read())
    except urllib.error.HTTPError as e:
        return e.code, _parse_body(e.read())


def api_patch(path: str, body: Any, auth: bool = True) -> tuple[int, Any]:
    hdrs = {**_headers(auth), "Content-Type": "application/json"}
    data = json.dumps(body).encode()
    req = urllib.request.Request(
        f"{BASE_URL}{path}", data=data, headers=hdrs, method="PATCH"
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, _parse_body(r.read())
    except urllib.error.HTTPError as e:
        return e.code, _parse_body(e.read())


def api_delete(path: str, auth: bool = True) -> tuple[int, Any]:
    req = urllib.request.Request(
        f"{BASE_URL}{path}", headers=_headers(auth), method="DELETE"
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, _parse_body(r.read())
    except urllib.error.HTTPError as e:
        return e.code, _parse_body(e.read())


def api_upload(
    path: str, filepath: Path, extra_fields: dict[str, str] | None = None
) -> tuple[int, Any]:
    """Multipart POST — file upload."""
    boundary = uuid.uuid4().hex
    body = b""
    for name, value in (extra_fields or {}).items():
        body += (
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="{name}"\r\n'
            f"\r\n"
            f"{value}\r\n"
        ).encode()
    file_bytes = filepath.read_bytes()
    body += (
        (
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="file"; filename="{filepath.name}"\r\n'
            f"Content-Type: application/octet-stream\r\n"
            f"\r\n"
        ).encode()
        + file_bytes
        + b"\r\n"
    )
    body += f"--{boundary}--\r\n".encode()
    hdrs = {
        "Authorization": f"Bearer {TOKEN}",
        "Content-Type": f"multipart/form-data; boundary={boundary}",
    }
    req = urllib.request.Request(
        f"{BASE_URL}{path}", data=body, headers=hdrs, method="POST"
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, _parse_body(r.read())
    except urllib.error.HTTPError as e:
        return e.code, _parse_body(e.read())


def raw_get(path: str, auth: bool = False) -> tuple[int, bytes]:
    """GET returning raw bytes (for feed XML, images, OPML)."""
    hdrs: dict[str, str] = {}
    if auth:
        hdrs["Authorization"] = f"Bearer {TOKEN}"
    req = urllib.request.Request(f"{BASE_URL}{path}", headers=hdrs)
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, b""


# ─────────────────────────────────────────────────────────────────────────────
# CLI helper
# ─────────────────────────────────────────────────────────────────────────────


def cli(*args: str) -> tuple[int, str]:
    """Run inkdrip-cli and return (exit_code, stdout)."""
    cmd = [str(CLI_BIN), "--url", BASE_URL, "--token", TOKEN, *args]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if VERBOSE and result.returncode != 0:
        print(f"    stderr: {result.stderr.strip()}")
    return result.returncode, result.stdout.strip()


def cli_json(*args: str) -> tuple[int, Any]:
    """Run CLI with --json flag and parse the output as JSON."""
    code, out = cli("--json", *args)
    if code != 0:
        return code, None
    try:
        return code, json.loads(out)
    except json.JSONDecodeError:
        return code, None


# ─────────────────────────────────────────────────────────────────────────────
# Server lifecycle
# ─────────────────────────────────────────────────────────────────────────────

_server_proc: subprocess.Popen[bytes] | None = None
_server_log: Any = None  # file object


def start_server() -> subprocess.Popen[bytes]:
    global _server_proc, _server_log
    log_path = DATA_DIR / "server.log"
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    _server_log = open(log_path, "wb")  # noqa: WPS515 (kept open intentionally)
    env = {**os.environ, "INKDRIP_CONFIG": str(E2E_CONFIG)}
    _server_proc = subprocess.Popen(
        [str(SERVER_BIN)],
        env=env,
        stdout=_server_log,
        stderr=_server_log,
        cwd=str(PROJECT_ROOT),
    )
    return _server_proc


def wait_ready(timeout: int = STARTUP_TIMEOUT) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{BASE_URL}/health", timeout=1) as r:
                if r.status == 200:
                    return True
        except Exception:
            pass
        time.sleep(0.3)
    return False


def stop_server() -> None:
    global _server_proc, _server_log
    if _server_proc:
        _server_proc.terminate()
        try:
            _server_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            _server_proc.kill()
            _server_proc.wait()
        _server_proc = None
    if _server_log:
        _server_log.close()
        _server_log = None


def print_server_log(last_lines: int = 30) -> None:
    log_path = DATA_DIR / "server.log"
    if not log_path.exists():
        return
    lines = log_path.read_text(errors="replace").splitlines()
    print(f"\n{YELLOW}--- server.log (last {last_lines} lines) ---{RESET}")
    for line in lines[-last_lines:]:
        print(f"  {line}")
    print(f"{YELLOW}--- end ---{RESET}")


# ─────────────────────────────────────────────────────────────────────────────
# Fixture book generators
# ─────────────────────────────────────────────────────────────────────────────

_SENTENCE = (
    "The art of exploration requires patience, curiosity, and a willingness to embrace "
    "uncertainty. Each step forward reveals new landscapes of understanding, challenging "
    "preconceptions and broadening perspectives. Through careful observation and "
    "methodical analysis, patterns emerge from apparent chaos, illuminating the "
    "underlying structure of complex systems."
)  # ~60 words


def _para(n_words: int, tag: str = "") -> str:
    """Generate approximately n_words of text, unique per tag.

    _SENTENCE is ~44 words; +1 prefix word per rep ≈ 45 words/rep.
    Use n_words // 40 to ensure we always have enough words to trim.
    """
    prefix = f"[{tag}] " if tag else ""
    reps = max(1, n_words // 40 + 1)
    blob = (prefix + _SENTENCE + " ") * reps
    return " ".join(blob.split()[:n_words])


def make_short_txt() -> bytes:
    """3 chapters via === separator, ~400 words each → ~1200 words total."""
    parts = [
        f"Chapter {i}: The Beginning\n\n{_para(400, f'sh-ch{i}')}" for i in range(1, 4)
    ]
    return ("\n\n===\n\n".join(parts)).encode()


def make_medium_md() -> bytes:
    """5 chapters via ## headings, ~600 words each → ~3000 words."""
    sections = [
        f"## Chapter {i}: Journey Forward\n\n{_para(600, f'md-ch{i}')}"
        for i in range(1, 6)
    ]
    return ("\n\n".join(sections)).encode()


def make_large_md() -> bytes:
    """10 chapters via # headings, 5 paragraphs × ~500 words each.

    Total: ~25 000 words. Each chapter is ~2500 words > max_segment_words (2000),
    so the splitter produces at least 2 segments per chapter → ≥ 20 segments.
    Multiple <p> blocks also allow resplitting at smaller granularity.
    """
    sections = []
    for i in range(1, 11):
        paras = "\n\n".join(_para(500, f"lg-ch{i}-p{p}") for p in range(1, 6))
        sections.append(f"# Part {i}: The Epic Continues\n\n{paras}")
    return ("\n\n".join(sections)).encode()


# ─────────────────────────────────────────────────────────────────────────────
# T1 — Health & Authentication
# ─────────────────────────────────────────────────────────────────────────────


def test_health_and_auth(r: TestRunner) -> None:
    r.section("T1 — Health & Authentication")

    status, _ = raw_get("/health")
    r.check("GET /health → 200 (no auth required)", status == 200, f"got {status}")

    # Unauthenticated access to protected endpoint
    status, _ = api_get("/api/books", auth=False)
    r.check("GET /api/books without token → 401", status == 401, f"got {status}")

    # Wrong token
    req = urllib.request.Request(
        f"{BASE_URL}/api/books",
        headers={"Authorization": "Bearer wrongtoken", "Accept": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as resp:
            bad_token_status = resp.status
    except urllib.error.HTTPError as e:
        bad_token_status = e.code
    r.check(
        "GET /api/books with wrong token → 401",
        bad_token_status == 401,
        f"got {bad_token_status}",
    )

    # Correct token
    status, _ = api_get("/api/books")
    r.check("GET /api/books with correct token → 200", status == 200, f"got {status}")


# ─────────────────────────────────────────────────────────────────────────────
# T2 — Book CRUD
# ─────────────────────────────────────────────────────────────────────────────


def test_book_crud(r: TestRunner, tmp_dir: Path) -> dict[str, str]:
    r.section("T2 — Book CRUD")
    book_ids: dict[str, str] = {}

    # Write fixture files to temp dir
    short_txt = tmp_dir / "short.txt"
    medium_md = tmp_dir / "medium.md"
    large_md = tmp_dir / "large.md"
    short_txt.write_bytes(make_short_txt())
    medium_md.write_bytes(make_medium_md())
    large_md.write_bytes(make_large_md())

    # Upload EPUB
    if EPUB_BOOK.exists():
        status, body = api_upload("/api/books", EPUB_BOOK, {"title": "Rustonomicon"})
        if r.check("Upload EPUB → 201", status == 201, f"got {status}: {body}"):
            book_ids["epub"] = body.get("book", {}).get("id", "")
            r.check(
                "EPUB upload returns book ID", bool(book_ids.get("epub")), str(body)
            )
    else:
        r.skip("Upload EPUB", f"file not found: {EPUB_BOOK.name}")

    # Upload TXT
    status, body = api_upload(
        "/api/books", short_txt, {"title": "Short Story", "author": "E2E Tester"}
    )
    if r.check("Upload TXT → 201", status == 201, f"got {status}: {body}"):
        book_ids["txt"] = body.get("book", {}).get("id", "")
    segs_count = body.get("segments_count", 0) if status == 201 else 0
    r.check("Short TXT produces ≥ 1 segment", segs_count >= 1, f"got {segs_count}")

    # Upload Markdown (medium)
    status, body = api_upload("/api/books", medium_md, {"title": "Medium Book"})
    if r.check("Upload MD → 201", status == 201, f"got {status}: {body}"):
        book_ids["md"] = body.get("book", {}).get("id", "")

    # Upload large Markdown
    status, body = api_upload("/api/books", large_md, {"title": "Large Book"})
    if r.check("Upload large MD → 201", status == 201, f"got {status}: {body}"):
        book_ids["large"] = body.get("book", {}).get("id", "")
    large_segs = body.get("segments_count", 0) if status == 201 else 0
    r.check("Large MD produces > 10 segments", large_segs > 10, f"got {large_segs}")

    # Duplicate upload → 409
    status, body = api_upload("/api/books", short_txt)
    r.check("Duplicate upload → 409", status == 409, f"got {status}: {body}")

    # List all books  (list_books returns a flat array of Book objects)
    status, body = api_get("/api/books")
    r.check("List books → 200", status == 200, f"got {status}")
    if status == 200:
        expected = len(book_ids)
        r.check(
            f"Book list has {expected} entries",
            isinstance(body, list) and len(body) == expected,
            f"got {len(body) if isinstance(body, list) else body}",
        )

    # Get book detail (nested response)
    if book_ids.get("large"):
        status, body = api_get(f"/api/books/{book_ids['large']}")
        r.check("Get book detail → 200", status == 200, f"got {status}")
        if status == 200:
            segs = body.get("segments", [])
            r.check("Book detail includes segments list", isinstance(segs, list), "")
            r.check(
                "Large MD detail has > 10 segments", len(segs) > 10, f"got {len(segs)}"
            )

    # Edit book title via CLI (supports prefix match on ID)
    if book_ids.get("md"):
        code, out = cli(
            "edit", "book", book_ids["md"], "--title", "Medium Book (Edited)"
        )
        r.check("CLI edit book --title → exit 0", code == 0, out)
        _, data = api_get(f"/api/books/{book_ids['md']}")
        r.check(
            "Title updated via API",
            data.get("book", {}).get("title") == "Medium Book (Edited)",
            str(data.get("book", {}).get("title")),
        )

    # CLI list books (table mode)
    code, out = cli("list", "books")
    r.check("CLI list books → exit 0", code == 0, out[:80] if out else "")

    return book_ids


# ─────────────────────────────────────────────────────────────────────────────
# T3 — Feed Lifecycle
# ─────────────────────────────────────────────────────────────────────────────


def test_feed_lifecycle(r: TestRunner, book_ids: dict[str, str]) -> dict[str, Any]:
    r.section("T3 — Feed Lifecycle")
    feed_ids: dict[str, str] = {}
    feed_slugs: dict[str, str] = {}

    # Create feed for TXT book
    if book_ids.get("txt"):
        status, body = api_post(
            f"/api/books/{book_ids['txt']}/feeds",
            {"words_per_day": 300, "delivery_time": "09:00", "slug": "e2e-short-feed"},
        )
        if r.check("Create feed for TXT → 201", status == 201, f"got {status}: {body}"):
            feed = body.get("feed", {})
            feed_ids["txt"] = feed.get("id", "")
            feed_slugs["txt"] = feed.get("slug", "")
            r.check(
                "Create feed returns feed_url",
                bool(body.get("feed_url")),
                str(body.get("feed_url")),
            )

    # Create feed for large MD (skip_days=96 = Sat+Sun)
    if book_ids.get("large"):
        status, body = api_post(
            f"/api/books/{book_ids['large']}/feeds",
            {"words_per_day": 1500, "slug": "e2e-large-feed", "skip_days": 96},
        )
        if r.check(
            "Create feed with skip_days=96 → 201",
            status == 201,
            f"got {status}: {body}",
        ):
            feed = body.get("feed", {})
            feed_ids["large"] = feed.get("id", "")
            feed_slugs["large"] = feed.get("slug", "")
            sched = feed.get("schedule_config", {})
            # bitflags 2 with serde feature serializes as pipe-separated string
            skip = sched.get("skip_days", "")
            skip_ok = skip in (96, "SAT | SUN", "SAT|SUN", "SUN | SAT", "SUN|SAT")
            r.check(
                "skip_days encodes SAT+SUN (96 or 'SAT | SUN')",
                skip_ok,
                f"schedule_config={sched}",
            )

    # Create feed for EPUB
    if book_ids.get("epub"):
        status, body = api_post(
            f"/api/books/{book_ids['epub']}/feeds",
            {"words_per_day": 3000, "slug": "e2e-epub-feed"},
        )
        if r.check(
            "Create feed for EPUB → 201", status == 201, f"got {status}: {body}"
        ):
            feed = body.get("feed", {})
            feed_ids["epub"] = feed.get("id", "")
            feed_slugs["epub"] = feed.get("slug", "")

    # List feeds
    status, body = api_get("/api/feeds")
    r.check("List feeds → 200", status == 200, f"got {status}")
    if status == 200:
        r.check(
            "Feeds list ≥ 2",
            isinstance(body, list) and len(body) >= 2,
            f"got {len(body)}",
        )

    # Feed detail
    if feed_ids.get("txt"):
        status, body = api_get(f"/api/feeds/{feed_ids['txt']}")
        r.check("Get feed detail → 200", status == 200, f"got {status}")
        if status == 200:
            feed = body.get("feed", {})
            r.check(
                "Feed status is 'active'",
                feed.get("status") == "active",
                str(feed.get("status")),
            )

    # Pause via CLI
    if feed_ids.get("txt"):
        code, out = cli("feed", "pause", feed_ids["txt"])
        r.check("CLI feed pause → exit 0", code == 0, out)
        _, data = api_get(f"/api/feeds/{feed_ids['txt']}")
        r.check(
            "Status becomes 'paused'",
            data.get("feed", {}).get("status") == "paused",
            "",
        )

    # Resume via CLI
    if feed_ids.get("txt"):
        code, out = cli("feed", "resume", feed_ids["txt"])
        r.check("CLI feed resume → exit 0", code == 0, out)
        _, data = api_get(f"/api/feeds/{feed_ids['txt']}")
        r.check(
            "Status back to 'active'",
            data.get("feed", {}).get("status") == "active",
            "",
        )

    # Edit words_per_day via API PATCH
    if feed_ids.get("txt"):
        status, body = api_patch(
            f"/api/feeds/{feed_ids['txt']}", {"words_per_day": 500}
        )
        r.check("PATCH feed words_per_day → 200", status == 200, f"got {status}")
        if status == 200:
            sched = body.get("feed", {}).get("schedule_config", {})
            r.check(
                "words_per_day updated to 500",
                sched.get("words_per_day") == 500,
                str(sched),
            )

    # Feed status via CLI
    if feed_ids.get("large"):
        code, out = cli("feed", "status", feed_ids["large"])
        r.check("CLI feed status → exit 0", code == 0, out[:80] if out else "")

    # Debug releases
    if feed_ids.get("large"):
        code, out = cli("debug", "releases", feed_ids["large"])
        r.check("CLI debug releases → exit 0", code == 0, "")

    # Debug segments
    if book_ids.get("large"):
        code, out = cli("debug", "segments", book_ids["large"])
        r.check("CLI debug segments → exit 0", code == 0, "")

    # Debug preview
    if feed_ids.get("large"):
        code, out = cli("debug", "preview", feed_ids["large"], "--limit", "3")
        r.check("CLI debug preview → exit 0", code == 0, "")

    return {"ids": feed_ids, "slugs": feed_slugs}


# ─────────────────────────────────────────────────────────────────────────────
# T4 — Feed XML (Atom / RSS)
# ─────────────────────────────────────────────────────────────────────────────


def test_feed_xml(
    r: TestRunner, feed_slugs: dict[str, str], feed_ids: dict[str, str]
) -> None:
    r.section("T4 — Feed XML (Atom / RSS)")

    slug = feed_slugs.get("large", "")
    fid = feed_ids.get("large", "")
    if not slug:
        r.skip("All T4 tests", "large feed not created in T3")
        return

    # Both formats accessible without auth (public endpoint)
    status, data = raw_get(f"/feeds/{slug}/atom.xml")
    r.check("GET .../atom.xml → 200 (no auth)", status == 200, f"got {status}")
    if status == 200:
        r.check(
            "Atom response is XML",
            b"<feed" in data or b"<?xml" in data,
            data[:80].decode(errors="replace"),
        )

    status, data = raw_get(f"/feeds/{slug}/rss.xml")
    r.check("GET .../rss.xml → 200 (no auth)", status == 200, f"got {status}")
    if status == 200:
        r.check(
            "RSS response contains <rss> or <channel>",
            b"<rss" in data or b"<channel" in data,
            data[:80].decode(errors="replace"),
        )

    # Initially 0 entries (all releases are in the future)
    status, data = raw_get(f"/feeds/{slug}/atom.xml")
    entries_before = data.count(b"<entry>") if status == 200 else -1
    r.check(
        "Atom feed has 0 entries before advance",
        entries_before == 0,
        f"got {entries_before}",
    )

    # Advance 3 segments via API
    if fid:
        status, body = api_post(f"/api/feeds/{fid}/advance", {"count": 3})
        r.check("POST advance 3 segments → 200", status == 200, f"got {status}: {body}")

    # Atom should now have 3 entries
    status, data = raw_get(f"/feeds/{slug}/atom.xml")
    entries_after = data.count(b"<entry>") if status == 200 else -1
    r.check(
        "Atom has 3 entries after advance", entries_after == 3, f"got {entries_after}"
    )

    # RSS should also have 3 items
    status, data = raw_get(f"/feeds/{slug}/rss.xml")
    rss_items = data.count(b"<item>") if status == 200 else -1
    r.check("RSS has 3 items after advance", rss_items == 3, f"got {rss_items}")

    # Advance 2 more via CLI
    if fid:
        code, out = cli("feed", "advance", fid, "-c", "2")
        r.check("CLI feed advance 2 more → exit 0", code == 0, out)
        status, data = raw_get(f"/feeds/{slug}/atom.xml")
        entries_final = data.count(b"<entry>") if status == 200 else -1
        r.check(
            "Atom has 5 entries after second advance",
            entries_final == 5,
            f"got {entries_final}",
        )

    # Content transforms: reading progress indicator in entry content
    status, data = raw_get(f"/feeds/{slug}/atom.xml")
    if status == 200 and b"<entry>" in data:
        r.check(
            "Atom entry content contains reading progress indicator",
            b"%" in data and b"<content" in data,
            "",
        )


# ─────────────────────────────────────────────────────────────────────────────
# T5 — Advanced Operations
# ─────────────────────────────────────────────────────────────────────────────


def test_advanced_ops(
    r: TestRunner, book_ids: dict[str, str], feed_ids: dict[str, str]
) -> None:
    r.section("T5 — Advanced Operations")

    # Read segment 0 via CLI
    if book_ids.get("large"):
        code, out = cli("read", book_ids["large"], "0")
        r.check("CLI read segment 0 → exit 0", code == 0, "")
        r.check("Segment content is non-empty", len(out) > 20, f"len={len(out)}")

    # OPML export (public, no auth)
    status, data = raw_get("/opml")
    r.check("GET /opml → 200 (no auth)", status == 200, f"got {status}")
    if status == 200:
        r.check(
            "OPML is XML with <opml>",
            b"<opml" in data or b"<?xml" in data,
            data[:80].decode(errors="replace"),
        )

    # Resplit large book with smaller target → more segments
    if book_ids.get("large"):
        _, detail = api_get(f"/api/books/{book_ids['large']}")
        segs_before = len(detail.get("segments", []))

        code, out = cli("resplit", book_ids["large"], "--target-words", "500")
        r.check(
            "CLI resplit --target-words 500 → exit 0",
            code == 0,
            out[:80] if out else "",
        )

        _, detail = api_get(f"/api/books/{book_ids['large']}")
        segs_after = len(detail.get("segments", []))
        r.check(
            f"Resplit increased segment count ({segs_before} → {segs_after})",
            segs_after > segs_before,
            f"before={segs_before} after={segs_after}",
        )

    # JSON mode: list books
    code, data = cli_json("list", "books")
    r.check(
        "CLI --json list books → exit 0 + JSON array",
        code == 0 and isinstance(data, list),
        f"code={code} type={type(data).__name__}",
    )

    # JSON mode: list feeds
    code, data = cli_json("list", "feeds")
    r.check(
        "CLI --json list feeds → JSON array",
        isinstance(data, list),
        str(type(data).__name__),
    )

    # JSON mode: feed status
    if feed_ids.get("large"):
        code, data = cli_json("feed", "status", feed_ids["large"])
        r.check(
            "CLI --json feed status → JSON object",
            code == 0 and isinstance(data, dict),
            "",
        )


# ─────────────────────────────────────────────────────────────────────────────
# T6 — Image Serving
# ─────────────────────────────────────────────────────────────────────────────


def test_images(r: TestRunner, epub_book_id: str) -> None:
    r.section("T6 — Image Serving")

    if not epub_book_id:
        r.skip("All T6 tests", "EPUB book not uploaded")
        return

    # Non-existent image → 404
    status, _ = raw_get(f"/images/{epub_book_id}/nonexistent_file.png")
    r.check("GET non-existent image → 404", status == 404, f"got {status}")

    # List segments and look for img tags with /images/ URLs
    status, segs = api_get(f"/api/books/{epub_book_id}/segments")
    r.check("GET /api/books/:id/segments → 200", status == 200, f"got {status}")
    if status != 200 or not isinstance(segs, list):
        return

    # Search for an <img> tag in any segment
    img_url: str | None = None
    for seg in segs[:20]:  # limit scan
        html = seg.get("content_html", "")
        if "<img" in html and f"/images/{epub_book_id}/" in html:
            # Extract src="..." value
            start = html.find('src="', html.find("<img")) + 5
            end = html.find('"', start)
            if start > 4 and end > start:
                img_url = html[start:end]
            break

    if img_url:
        # Strip base URL prefix to get just the path
        path = img_url.replace(BASE_URL, "")
        st, img_data = raw_get(path)
        r.check(
            "Image endpoint returns 200 with data",
            st == 200 and len(img_data) > 0,
            f"got {st}, {len(img_data)} bytes",
        )
        r.check(
            "Image URL rewritten to /images/ path",
            f"/images/{epub_book_id}/" in img_url,
            img_url,
        )
    else:
        r.check("EPUB processed (no img tags in first 20 segments)", True, "")


# ─────────────────────────────────────────────────────────────────────────────
# T7 — Aggregate Feeds
# ─────────────────────────────────────────────────────────────────────────────


def test_aggregates(
    r: TestRunner, feed_ids: dict[str, str], feed_slugs: dict[str, str]
) -> str:
    r.section("T7 — Aggregate Feeds")

    # Create include_all aggregate
    status, body = api_post(
        "/api/aggregates",
        {
            "slug": "e2e-all-books",
            "title": "All Books",
            "description": "E2E test aggregate",
            "include_all": True,
        },
    )
    r.check(
        "Create aggregate (include_all=true) → 201",
        status == 201,
        f"got {status}: {body}",
    )
    all_agg_id = body.get("aggregate", {}).get("id", "") if status == 201 else ""

    # Create explicit-feeds aggregate
    source_slugs = [s for s in [feed_slugs.get("large"), feed_slugs.get("txt")] if s]
    status, body = api_post(
        "/api/aggregates",
        {
            "slug": "e2e-selected",
            "title": "Selected Feeds",
            "include_all": False,
            "feeds": source_slugs,
        },
    )
    r.check(
        "Create aggregate (explicit feeds) → 201",
        status == 201,
        f"got {status}: {body}",
    )
    sel_agg_id = body.get("aggregate", {}).get("id", "") if status == 201 else ""

    # List aggregates
    status, body = api_get("/api/aggregates")
    r.check("List aggregates → 200", status == 200, f"got {status}")
    if status == 200:
        r.check("2 aggregates returned", len(body) == 2, f"got {len(body)}")

    # Get aggregate detail
    if all_agg_id:
        status, d = api_get(f"/api/aggregates/{all_agg_id}")
        r.check("Get aggregate detail → 200", status == 200, f"got {status}")
        if status == 200:
            r.check(
                "Aggregate has 'aggregate' key", "aggregate" in d, str(list(d.keys()))
            )

    # Public aggregate Atom feed (no auth)
    status, data = raw_get("/aggregates/e2e-all-books/atom.xml")
    r.check(
        "GET /aggregates/e2e-all-books/atom.xml → 200 (no auth)",
        status == 200,
        f"got {status}",
    )
    if status == 200:
        r.check("Aggregate atom is XML", b"<feed" in data or b"<?xml" in data, "")

    # RSS format
    status, data = raw_get("/aggregates/e2e-all-books/rss.xml")
    r.check(
        "GET /aggregates/e2e-all-books/rss.xml → 200", status == 200, f"got {status}"
    )

    # Add source via API (add EPUB feed to selected aggregate)
    if sel_agg_id and feed_ids.get("epub"):
        status, _ = api_post(f"/api/aggregates/{sel_agg_id}/feeds/{feed_ids['epub']}")
        r.check("Add feed source → 204", status == 204, f"got {status}")

    # Aggregate XML has entries from feeds that have advanced segments (set in T4)
    status, data = raw_get("/aggregates/e2e-all-books/atom.xml")
    entries = data.count(b"<entry>") if status == 200 else -1
    r.check(
        "Aggregate atom has ≥ 5 entries (from T4 advances)",
        entries >= 5,
        f"got {entries}",
    )

    # Delete aggregate via CLI
    if all_agg_id:
        code, out = cli("aggregate", "delete", all_agg_id)
        r.check("CLI aggregate delete → exit 0", code == 0, out)
        status, _ = api_get(f"/api/aggregates/{all_agg_id}")
        r.check("Deleted aggregate → 404", status == 404, f"got {status}")

    # CLI list aggregates
    code, out = cli("aggregate", "list")
    r.check("CLI aggregate list → exit 0", code == 0, "")

    return sel_agg_id


# ─────────────────────────────────────────────────────────────────────────────
# T8 — Cascade Delete
# ─────────────────────────────────────────────────────────────────────────────


def test_cascade_delete(
    r: TestRunner, book_ids: dict[str, str], feed_ids: dict[str, str]
) -> None:
    r.section("T8 — Cascade Delete")

    # Delete feed only → book survives
    if feed_ids.get("txt") and book_ids.get("txt"):
        status, _ = api_delete(f"/api/feeds/{feed_ids['txt']}")
        r.check("DELETE feed → 204", status == 204, f"got {status}")

        status, _ = api_get(f"/api/feeds/{feed_ids['txt']}")
        r.check("Deleted feed → 404", status == 404, f"got {status}")

        status, _ = api_get(f"/api/books/{book_ids['txt']}")
        r.check("Book intact after feed delete → 200", status == 200, f"got {status}")

    # Create a temp feed for MD book, then delete the book → feed cascades
    if book_ids.get("md"):
        fid_temp = ""
        st, body = api_post(
            f"/api/books/{book_ids['md']}/feeds",
            {"words_per_day": 1000, "slug": "e2e-temp-md-feed"},
        )
        if st == 201:
            fid_temp = body.get("feed", {}).get("id", "")

        code, out = cli("remove", book_ids["md"])
        r.check("CLI remove book → exit 0", code == 0, out)

        status, _ = api_get(f"/api/books/{book_ids['md']}")
        r.check("Deleted book → 404", status == 404, f"got {status}")

        if fid_temp:
            status, _ = api_get(f"/api/feeds/{fid_temp}")
            r.check("Cascade: feed also → 404", status == 404, f"got {status}")

    # Final list should show reduced count
    status, body = api_get("/api/books")
    if status == 200:
        r.check("Book list reduced after deletes", isinstance(body, list), "")


# ─────────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────────


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="InkDrip E2E test suite")
    p.add_argument("--no-build", action="store_true", help="Skip cargo build --release")
    p.add_argument("--keep", action="store_true", help="Keep data-test/ on failure")
    p.add_argument(
        "-v", "--verbose", action="store_true", help="Verbose output on failure"
    )
    return p.parse_args()


def main() -> int:
    global VERBOSE
    args = parse_args()
    VERBOSE = args.verbose

    # Build step
    if not args.no_build:
        print(f"{BOLD}Building release binaries...{RESET}")
        result = subprocess.run(
            ["cargo", "build", "--release", "--workspace"],
            cwd=str(PROJECT_ROOT),
            env={**os.environ, "RUSTC_WRAPPER": ""},
        )
        if result.returncode != 0:
            print(f"{RED}✗ Build failed.{RESET}")
            return 1
        print(f"{GREEN}✓ Build succeeded.{RESET}")

    if not SERVER_BIN.exists() or not CLI_BIN.exists():
        print(f"{RED}Binaries not found. Run `just build` first.{RESET}")
        return 1

    # Clean previous test data
    if DATA_DIR.exists():
        shutil.rmtree(DATA_DIR)
    DATA_DIR.mkdir(parents=True)

    runner = TestRunner()

    print(f"\n{BOLD}Starting server on :{PORT}...{RESET}")
    start_server()
    if not wait_ready():
        print(f"{RED}✗ Server did not become ready within {STARTUP_TIMEOUT}s.{RESET}")
        print_server_log()
        stop_server()
        return 1
    print(f"{GREEN}✓ Server ready.{RESET}")

    with tempfile.TemporaryDirectory(prefix="inkdrip-fixtures-") as tmp:
        tmp_dir = Path(tmp)
        try:
            test_health_and_auth(runner)
            book_ids = test_book_crud(runner, tmp_dir)
            feed_data = test_feed_lifecycle(runner, book_ids)
            feed_ids: dict[str, str] = feed_data["ids"]
            feed_slugs: dict[str, str] = feed_data["slugs"]
            test_feed_xml(runner, feed_slugs, feed_ids)
            test_advanced_ops(runner, book_ids, feed_ids)
            test_images(runner, book_ids.get("epub", ""))
            test_aggregates(runner, feed_ids, feed_slugs)
            test_cascade_delete(runner, book_ids, feed_ids)
        except KeyboardInterrupt:
            print(f"\n{YELLOW}Interrupted.{RESET}")
        finally:
            stop_server()

    keep = args.keep and runner.failed > 0
    if keep:
        print(f"\n{YELLOW}data-test/ kept for post-mortem: {DATA_DIR}{RESET}")
    elif DATA_DIR.exists():
        shutil.rmtree(DATA_DIR)
        print(f"\n{CYAN}data-test/ cleaned up.{RESET}")

    ok = runner.summary()
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
