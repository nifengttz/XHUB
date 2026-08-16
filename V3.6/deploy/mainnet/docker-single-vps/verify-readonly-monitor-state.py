#!/usr/bin/env python3
import argparse
import json
import sqlite3
from pathlib import Path


def read_monitor_state(database: Path, funding_coin_id: str) -> dict[str, object]:
    connection = sqlite3.connect(f"file:{database.as_posix()}?mode=ro", uri=True)
    try:
        row = connection.execute(
            """
            SELECT lower(hex(funding_coin_id)), peak_height, action, detail, updated_at
            FROM v36_chain_monitor_state
            WHERE lower(hex(funding_coin_id)) = ?
            """,
            (funding_coin_id,),
        ).fetchone()
    finally:
        connection.close()

    if row is None:
        raise RuntimeError(f"monitor state missing from {database}")

    return {
        "database": str(database),
        "funding_coin_id": row[0],
        "peak_height": row[1],
        "action": row[2],
        "detail": row[3],
        "updated_at": row[4],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("funding_coin_id")
    parser.add_argument("databases", nargs="+")
    args = parser.parse_args()

    funding_coin_id = args.funding_coin_id.removeprefix("0x").lower()
    if len(funding_coin_id) != 64:
        raise ValueError("funding_coin_id must contain exactly 32 bytes")

    states = [
        read_monitor_state(Path(database), funding_coin_id)
        for database in args.databases
    ]
    print(json.dumps({"states": states}, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
