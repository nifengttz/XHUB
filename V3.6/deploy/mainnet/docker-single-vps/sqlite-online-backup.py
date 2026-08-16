#!/usr/bin/env python3
import json
import sqlite3
import sys
from pathlib import Path
from urllib.parse import quote


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: sqlite-online-backup.py SOURCE DESTINATION")
    source_path = Path(sys.argv[1]).resolve()
    destination_path = Path(sys.argv[2]).resolve()
    if not source_path.is_file():
        raise SystemExit("source SQLite database does not exist")
    if destination_path.exists():
        raise SystemExit("destination already exists")

    source_uri = f"file:{quote(str(source_path))}?mode=ro"
    with sqlite3.connect(source_uri, uri=True) as source:
        with sqlite3.connect(destination_path) as destination:
            source.backup(destination)
            quick_check = destination.execute("PRAGMA quick_check").fetchone()[0]
            event_count = destination.execute(
                "SELECT COUNT(*) FROM v36_monitor_alert_events"
            ).fetchone()[0]
    if quick_check != "ok":
        raise SystemExit(f"SQLite quick_check failed: {quick_check}")
    print(
        json.dumps(
            {
                "schema": "xhub-v3-6-sqlite-online-backup-report-1",
                "quick_check": quick_check,
                "event_count": event_count,
            },
            separators=(",", ":"),
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
