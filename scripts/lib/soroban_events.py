#!/usr/bin/env python3
"""BettaPay — shared Soroban event reading/decoding helpers.

Used by ``scripts/export_events.sh`` and ``scripts/list_pending_ops.sh``.
The shell scripts own argument parsing and user-facing documentation, and
forward parsed flags here (see the ``export`` and ``pending`` subcommands).

It reads a Stellar/Soroban RPC server's ``getEvents`` JSON-RPC method,
normalises the events, decodes ``topic[0]`` as an XDR ``SCVal`` symbol and
validates it against the event-name constants declared in
``bettapay_common/src/events.rs`` (the single source of truth for topic
names), so exporters and indexers cannot drift from the contracts.
"""

import argparse
import base64
import binascii
import json
import re
import struct
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

# ---- SCVal XDR discriminants (stellar_xdr::curr::SCValType) ---------------
SCVAL_BOOL = 0
SCVAL_U32 = 2
SCVAL_I32 = 3
SCVAL_U64 = 4
SCVAL_I64 = 5
SCVAL_TIMEPOINT = 6
SCVAL_DURATION = 7
SCVAL_U128 = 8
SCVAL_I128 = 9
SCVAL_BYTES = 10
SCVAL_STRING = 11
SCVAL_SYMBOL = 12
SCVAL_VEC = 13
SCVAL_MAP = 14
SCVAL_ADDRESS = 15

DEFAULT_PAGE_LIMIT = 200
DEFAULT_WINDOW_LEDGERS = 1000
DEFAULT_PENDING_WINDOW_LEDGERS = 150000  # ~7 days at ~5s/ledger

OP_SCHEDULED = "op_scheduled"
OP_EXECUTED = "op_executed"
OP_CANCELLED = "op_cancelled"


class RpcError(Exception):
    pass


# ---- XDR primitives --------------------------------------------------------

def _pad(length: int) -> int:
    return (4 - (length % 4)) % 4


class Reader:
    """Cursor over a bytes object for big-endian XDR decoding."""

    def __init__(self, data: bytes):
        self.data = data
        self.pos = 0

    def take(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise ValueError("truncated XDR: wanted %d bytes at offset %d of %d"
                             % (n, self.pos, len(self.data)))
        out = self.data[self.pos:self.pos + n]
        self.pos += n
        return out

    def u32(self) -> int:
        return struct.unpack(">I", self.take(4))[0]

    def i32(self) -> int:
        return struct.unpack(">i", self.take(4))[0]

    def u64(self) -> int:
        return struct.unpack(">Q", self.take(8))[0]

    def i64(self) -> int:
        return struct.unpack(">q", self.take(8))[0]

    def opaque(self) -> bytes:
        n = self.u32()
        payload = self.take(n)
        self.take(_pad(n))
        return payload

    def u128(self) -> int:
        return (self.u64() << 64) | self.u64()

    def i128(self) -> int:
        return (self.i64() << 64) | self.u64()


def encode_symbol(symbol: str) -> str:
    """Base64 XDR encoding of an SCSymbol, for getEvents topic filters."""
    raw = symbol.encode("utf-8")
    body = struct.pack(">I", SCVAL_SYMBOL) + struct.pack(">I", len(raw)) + raw
    body += b"\x00" * _pad(len(raw))
    return base64.b64encode(body).decode("ascii")


def decode_symbol(topic_b64: str) -> str:
    """Decode a base64 XDR SCVal that should be a symbol."""
    raw = base64.b64decode(topic_b64)
    reader = Reader(raw)
    tag = reader.u32()
    if tag != SCVAL_SYMBOL:
        raise ValueError("value is not a symbol (SCVal tag %d)" % tag)
    return reader.opaque().decode("utf-8")


def is_symbol(topic_b64: str) -> bool:
    try:
        raw = base64.b64decode(topic_b64)
        return (len(raw) >= 4
                and struct.unpack(">I", raw[:4])[0] == SCVAL_SYMBOL)
    except (binascii.Error, struct.error):
        return False


def decode_bytes_n(topic_b64: str):
    """Decode a base64 XDR SCVal that should be Bytes, as hex."""
    raw = base64.b64decode(topic_b64)
    reader = Reader(raw)
    if reader.u32() != SCVAL_BYTES:
        raise ValueError("value is not Bytes (SCVal discriminant mismatch)")
    return reader.opaque().hex()


def strkey_encode(version: int, payload: bytes) -> str:
    """Stellar StrKey encoding (base32 payload + version + CRC16/XModem)."""

    def crc16_xmodem(data: bytes) -> int:
        crc = 0x0000
        for byte in data:
            crc ^= byte << 8
            for _ in range(8):
                crc = ((crc << 1) ^ 0x1021) if (crc & 0x8000) else (crc << 1)
                crc &= 0xFFFF
        return crc

    body = bytes([version]) + payload
    checksum = struct.pack("<H", crc16_xmodem(body))
    blob = body + checksum
    return base64.b32encode(blob).decode("ascii").rstrip("=")


def decode_address(raw: bytes) -> str:
    """Pretty-render an XDR SCAddress body: G... or C... / raw envelope.

    ``raw`` starts right after the 4-byte SCAddressType discriminant.
    """
    reader = Reader(raw)
    if reader.u32() == 0:  # SC_ADDRESS_TYPE_ACCOUNT
        # AccountId body: 4-byte keytype (Ed25519 = 0) + 32-byte public key
        if reader.u32() != 0:
            return "<unsupported account key>"
        return strkey_encode(6 << 3, reader.take(32))
    if len(raw) >= 4 + 32:  # SC_ADDRESS_TYPE_CONTRACT
        return strkey_encode(24 << 3, raw[4:36])
    return "<unsupported address>"


def decode_scval(reader: Reader):
    """Best-effort recursive SCVal decode into plain python values."""
    tag = reader.u32()
    if tag == SCVAL_BOOL:
        return reader.u32() == 1
    if tag == SCVAL_U32:
        return reader.u32()
    if tag == SCVAL_I32:
        return reader.i32()
    if tag in (SCVAL_U64, SCVAL_DURATION):
        return reader.u64()
    if tag in (SCVAL_I64, SCVAL_TIMEPOINT):
        return reader.i64()
    if tag == SCVAL_U128:
        return reader.u128()
    if tag == SCVAL_I128:
        return reader.i128()
    if tag in (SCVAL_BYTES, SCVAL_STRING, SCVAL_SYMBOL):
        opaque = reader.opaque()
        if tag == SCVAL_BYTES:
            return opaque.hex()
        return opaque.decode("utf-8", errors="backslashreplace")
    if tag == SCVAL_VEC:
        return [decode_scval(reader) for _ in range(reader.u32())]
    if tag == SCVAL_MAP:
        mapping = {}
        for _ in range(reader.u32()):
            key = decode_scval(reader)
            mapping[key if isinstance(key, (str, int, bool)) else repr(key)] \
                = decode_scval(reader)
        return mapping
    if tag == SCVAL_ADDRESS:
        return decode_address(reader.data[reader.pos:])
    raise ValueError("unsupported SCVal[tag] %d" % tag)


def decode_event_value(value_b64: str):
    """Decode an event `value` field (base64 XDR SCVal) best-effort."""
    raw = base64.b64decode(value_b64)
    reader = Reader(raw)
    try:
        return decode_scval(reader)
    except (ValueError, IndexError):
        return None


# ---- bettapay_common event-name constants ---------------------------------

EVENT_NAME_RE = re.compile(r'pub const ([A-Z0-9_]+): &str = "([A-Za-z0-9_]+)"')


def load_event_names(events_rs_path):
    """Parse `pub const *_EVENT: &str = "..."` out of bettapay_common's events.rs."""
    try:
        with open(events_rs_path, "r", encoding="utf-8") as handle:
            text = handle.read()
    except OSError:
        return {}
    return {value: constant for constant, value in EVENT_NAME_RE.findall(text)
            if constant.endswith("_EVENT")}


# ---- RPC plumbing ----------------------------------------------------------

def rpc_request(rpc_url: str, method: str, params) -> dict:
    request = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method,
                          "params": params}).encode("utf-8")
    req = urllib.request.Request(
        rpc_url, data=request, headers={"Content-Type": "application/json"},
        method="POST")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        raise RpcError("RPC HTTP error %s: %s"
                       % (exc.code, exc.read().decode(
                           "utf-8", errors="backslashreplace")))
    except urllib.error.URLError as exc:
        raise RpcError("RPC connection failed: %s" % exc.reason)
    except binascii.Error as exc:
        raise RpcError("RPC response decode failed: %s" % exc)
    if "error" in payload:
        err = payload["error"]
        raise RpcError("RPC %s error %s: %s"
                       % (method, err.get("code"), err.get("message")))
    if "result" not in payload:
        raise RpcError("RPC %s returned no result" % method)
    return payload["result"]


def paging_key(token: str):
    # Soroban paging tokens look like "<left>-<right>" hex pairs.
    if isinstance(token, str):
        try:
            left, right = token.split("-", 1)
            return (int(left, 16), int(right, 16))
        except (ValueError, AttributeError):
            return (0, token)
    return (0, token)


def read_saved_events(from_file: str) -> list:
    """Load events from a saved raw getEvents response (offline replay)."""
    with open(from_file, "r", encoding="utf-8") as handle:
        payload = json.load(handle)
    if isinstance(payload, dict):
        if "result" in payload:
            payload = payload["result"]
        if isinstance(payload, dict) and "events" in payload:
            payload = payload["events"]
    if not isinstance(payload, list):
        raise RpcError("--from-file payload is not a getEvents response")
    return payload


def fetch_events(rpc_url, start_ledger, end_ledger, contract_id,
                 topic0_names=None, page_limit=DEFAULT_PAGE_LIMIT):
    """Page the getEvents endpoint over [start_ledger, end_ledger].

    ``end_ledger`` is enforced client-side: Soroban RPC's getEvents accepts a
    start ledger (or a paging cursor) but no upper bound, so the reader stops
    paging once it observes an event past ``end_ledger`` or an empty page.
    Events are returned sorted by (ledger, paging token) deterministically.
    """
    latest = int(rpc_request(rpc_url, "getLatestLedger", [])["sequence"])
    if start_ledger is None:
        window = (DEFAULT_PENDING_WINDOW_LEDGERS if topic0_names
                  else DEFAULT_WINDOW_LEDGERS)
        start_ledger = max(0, latest - window)
    if end_ledger is None or end_ledger > latest:
        end_ledger = latest

    if topic0_names:
        filters = [{"type": "contract", "contractIds": [contract_id],
                    "topic": [encode_symbol(name)]} for name in topic0_names]
    else:
        filters = [{"type": "contract", "contractIds": [contract_id]}]

    events = []
    cursor = None
    while True:
        params = {"filters": filters, "pagination": {"limit": page_limit}}
        if cursor is not None:
            params["pagination"] = {"cursor": cursor, "limit": page_limit}
        else:
            params["startLedger"] = int(start_ledger)
        try:
            result = rpc_request(rpc_url, "getEvents", params)
        except RpcError as exc:
            # Paging past retained ledger history manifests as a pagination
            # error; treat it as the natural end of the requested range.
            if cursor is not None and ("start node" in str(exc)
                                       or "not found" in str(exc)
                                       or "cursor" in str(exc).lower()):
                break
            raise
        page = result.get("events", []) or []
        if not page:
            break
        stop = False
        for event in page:
            if int(event.get("ledger", 0)) > end_ledger:
                stop = True
                break
            events.append(event)
        cursor = page[-1].get("pagingToken")
        if stop or cursor is None or len(page) < page_limit:
            break
    events.sort(key=lambda e: (int(e.get("ledger", 0)),
                               paging_key(e.get("pagingToken", ""))))
    return events, int(start_ledger), int(end_ledger)


def normalise_event(event: dict, event_names: dict) -> dict:
    topics = event.get("topic", []) or []
    try:
        topic0_symbol = decode_symbol(topics[0]) if topics else ""
    except (ValueError, binascii.Error):
        topic0_symbol = "<undecodable>" if topics else ""
    topic1 = ""
    if len(topics) > 1:
        if is_symbol(topics[1]):
            try:
                topic1 = decode_symbol(topics[1])
            except (ValueError, binascii.Error):
                topic1 = "<undecodable>"
        else:
            topic1 = topics[1]  # raw base64 XDR (e.g. a hash topic)
    return {
        "ledger": int(event.get("ledger", 0)),
        "paging_token": event.get("pagingToken", ""),
        "ledger_timestamp": event.get("ledgerClosedAt", ""),
        "contract_id": event.get("contractId", ""),
        "event_type": event.get("type", "contract"),
        "topic0": topic0_symbol,
        "topic0_constant": event_names.get(topic0_symbol),
        "topic_count": len(topics),
        "topic1": topic1,
        "data_base64": event.get("value", ""),
        "data_decoded": decode_event_value(event.get("value", "")),
    }


# ---- CSV/JSON output -------------------------------------------------------

CSV_COLUMNS = ["ledger", "paging_token", "ledger_timestamp", "contract_id",
               "event_type", "event_name", "topic0", "topic1", "data"]


def csv_field(value) -> str:
    if value is None:
        return ""
    text = value if isinstance(value, str) else json.dumps(
        value, sort_keys=True, ensure_ascii=True)
    if "," in text or '"' in text or "\n" in text:
        return '"' + text.replace('"', '""') + '"'
    return text


def write_csv(records, stream):
    stream.write(",".join(CSV_COLUMNS) + "\n")
    for record in records:
        event_name = record["topic0"]
        if record["topic0_constant"]:
            event_name = record["topic0_constant"]
        stream.write(",".join([
            csv_field(record["ledger"]),
            csv_field(record["paging_token"]),
            csv_field(record["ledger_timestamp"]),
            csv_field(record["contract_id"]),
            csv_field(record["event_type"]),
            csv_field(event_name),
            csv_field(record["topic0"]),
            csv_field(record["topic1"]),
            csv_field(record["data_base64"]),
        ]) + "\n")


def write_json(records, stream, pretty=False):
    stream.write(json.dumps(records, indent=2 if pretty else None,
                            sort_keys=True, ensure_ascii=True))
    stream.write("\n")


# ---- pending operation reconstruction --------------------------------------

def extract_topic_symbols(event: dict):
    decoded = []
    for topic in event.get("topic", []) or []:
        try:
            decoded.append(decode_symbol(topic))
        except (ValueError, binascii.Error):
            decoded.append(None)
    return decoded


def extract_op_hash(event: dict):
    """Operation hash: the second topic, an XDR BytesN<32> SCVal."""
    topics = event.get("topic", []) or []
    if len(topics) < 2:
        return None
    try:
        return decode_bytes_n(topics[1])
    except (ValueError, binascii.Error):
        return None


def extract_scheduled_payload(event: dict):
    """op_scheduled data is `(caller: Address, execute_at: u64)`."""
    decoded = decode_event_value(event.get("value", ""))
    caller, execute_at = None, None
    if isinstance(decoded, list):
        for item in decoded:
            if caller is None and isinstance(item, str) and len(item) in (56,):
                caller = item
            elif execute_at is None and isinstance(item, int):
                execute_at = item
    return caller, execute_at


def collect_pending(events, contract_id=None):
    """Reconstruct the pending operation set from op_* life-cycle events."""
    scheduled = {}
    finished = {}
    for event in events:
        topic0 = (extract_topic_symbols(event) or [None])[0]
        if topic0 not in (OP_SCHEDULED, OP_EXECUTED, OP_CANCELLED):
            continue
        op_hash = extract_op_hash(event)
        if op_hash is None:
            continue
        if topic0 == OP_SCHEDULED:
            caller, execute_at = extract_scheduled_payload(event)
            scheduled[op_hash] = {
                "contract_id": event.get("contractId", ""),
                "op_hash": op_hash,
                "execute_at": execute_at,
                "caller": caller,
                "scheduled_ledger": int(event.get("ledger", 0)),
                "paging_token": event.get("pagingToken", ""),
            }
        else:
            finished[op_hash] = topic0
    pending = [entry for op_hash, entry in scheduled.items()
               if op_hash not in finished]
    if contract_id is not None:
        pending = [entry for entry in pending
                   if entry["contract_id"] == contract_id]
    pending.sort(key=lambda e: (e["execute_at"] if e["execute_at"] is not None
                                else 2**64, e["op_hash"]))
    return pending, len(scheduled), len(finished)


def format_due(execute_at: int, now: int) -> str:
    delta = execute_at - now
    if delta <= 0:
        return "ready"
    hours = delta // 3600
    minutes = (delta % 3600) // 60
    seconds = delta % 60
    if hours >= 24:
        return "%dd %dh %dm" % (hours // 24, hours % 24, minutes)
    if hours > 0:
        return "%dh %dm %ds" % (hours, minutes, seconds)
    if minutes > 0:
        return "%dm %ds" % (minutes, seconds)
    return "%ds" % seconds


def utc_readable(epoch: int) -> str:
    return datetime.fromtimestamp(int(epoch), tz=timezone.utc).strftime(
        "%Y-%m-%d %H:%M:%S UTC")


# ---- CLI -------------------------------------------------------------------

def _add_common_args(parser):
    parser.add_argument("--rpc-url", default=None)
    parser.add_argument("--contract-id", default=None)
    parser.add_argument("--start-ledger", default=None)
    parser.add_argument("--end-ledger", default=None)
    parser.add_argument("--page-limit", type=int, default=DEFAULT_PAGE_LIMIT)
    parser.add_argument("--events-src", default=None)
    parser.add_argument("--from-file", default=None)


def cmd_export(argv):
    parser = argparse.ArgumentParser(prog="soroban_events.py export")
    _add_common_args(parser)
    parser.add_argument("--format", choices=("json", "csv"), default="json")
    parser.add_argument("--pretty", action="store_true")
    args = parser.parse_args(argv)

    if args.contract_id is None:
        sys.stderr.write("--contract-id is required for export\n")
        return 2

    if args.from_file:
        raw_events = read_saved_events(args.from_file)
    else:
        if not args.rpc_url:
            sys.stderr.write(
                "--rpc-url is required unless --from-file is set\n")
            return 2
        start = int(args.start_ledger) if args.start_ledger else None
        end = int(args.end_ledger) if args.end_ledger else None
        raw_events, start, end = fetch_events(
            args.rpc_url, start, end, args.contract_id,
            topic0_names=None, page_limit=args.page_limit)

    event_names = load_event_names(args.events_src) if args.events_src else {}
    records = [normalise_event(event, event_names) for event in raw_events]
    records.sort(key=lambda r: (r["ledger"], paging_key(r["paging_token"])))

    if args.format == "csv":
        write_csv(records, sys.stdout)
    else:
        write_json(records, sys.stdout, pretty=args.pretty)
    return 0


def cmd_pending(argv):
    parser = argparse.ArgumentParser(prog="soroban_events.py pending")
    _add_common_args(parser)
    parser.add_argument("--format", choices=("table", "json"), default="table")
    parser.add_argument("--now", type=int, default=None)
    args = parser.parse_args(argv)

    if args.contract_id is None:
        sys.stderr.write("--contract-id is required\n")
        return 2

    if args.from_file:
        raw_events = read_saved_events(args.from_file)
    else:
        if not args.rpc_url:
            sys.stderr.write(
                "--rpc-url is required unless --from-file is set\n")
            return 2
        start = int(args.start_ledger) if args.start_ledger else None
        end = int(args.end_ledger) if args.end_ledger else None
        raw_events, start, end = fetch_events(
            args.rpc_url, start, end, args.contract_id,
            topic0_names=(OP_SCHEDULED, OP_EXECUTED, OP_CANCELLED),
            page_limit=args.page_limit)

    pending, scheduled_count, finished_count = collect_pending(raw_events,
                                                               args.contract_id)
    now = args.now if args.now is not None else int(time.time())
    for entry in pending:
        if entry["execute_at"] is not None:
            entry["execute_at_readable"] = utc_readable(entry["execute_at"])
            entry["due_in"] = format_due(entry["execute_at"], now)
            entry["overdue"] = entry["execute_at"] <= now

    if args.format == "json":
        write_json(pending, sys.stdout, pretty=True)
        return 0

    if not pending:
        sys.stdout.write(
            "No pending scheduled operations for %s\n"
            "(scanned: %d scheduled, %d executed/cancelled)\n"
            % (args.contract_id, scheduled_count, finished_count))
        return 0
    sys.stdout.write(
        "%-64s %-19s %-15s %s\n"
        % ("OPERATION HASH", "EXECUTE_AT (UTC)", "DUE IN", "CONTRACT"))
    for entry in pending:
        if entry["execute_at"] is None:
            readable, due = "unknown", "?"
        else:
            readable = utc_readable(entry["execute_at"])
            due = format_due(entry["execute_at"], now)
        sys.stdout.write("%-64s %-19s %-15s %s\n"
                         % (entry["op_hash"], readable, due,
                            entry["contract_id"]))
    return 0


def main():
    commands = {"export": cmd_export, "pending": cmd_pending}
    if len(sys.argv) < 2 or sys.argv[1] not in commands:
        sys.stderr.write("usage: soroban_events.py {export|pending} [args]\n")
        return 2
    return commands[sys.argv[1]](sys.argv[2:])


if __name__ == "__main__":
    raise SystemExit(main())
