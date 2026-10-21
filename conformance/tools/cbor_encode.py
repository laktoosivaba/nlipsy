#!/usr/bin/env python3
"""Encode a JSON description of a CBOR value to hex, for the NLIP conformance corpus.

The corpus records CBOR inputs as `input_hex`. The bytes have to be reproducible by
anyone auditing a case, so they are produced from a JSON description by this script
rather than typed by hand:

    python3 conformance/tools/cbor_encode.py value.json
    python3 conformance/tools/cbor_encode.py < value.json
    python3 conformance/tools/cbor_encode.py --prefix 12 value.json   # truncated frame

JSON cannot spell everything CBOR can, so four markers stand in for what it lacks:

    {"$bytes": "<standard base64>"}   a byte string (major type 2)
    {"$tag": [<number>, <value>]}     a tagged value (major type 6), e.g. [2, …] for
                                      the positive bignum of RFC 8949 §3.4.3
    {"$map": [[<key>, <value>], …]}   a map whose keys need not be text, which a JSON
                                      object cannot express (its names are strings)
    {"$float": "inf"}                 a non-finite IEEE 754 double (RFC 8949 §3.3):
                                      "inf", "-inf" or "nan". ECMA-404 has no spelling
                                      for these, which is the point of the fixtures
                                      that use them

Every other JSON value encodes as the obvious CBOR counterpart: object ->
definite-length map (major 5), array -> definite-length array (major 4), string ->
UTF-8 text string (major 3), integer -> unsigned/negative integer (major 0/1), float ->
IEEE 754 double (0xfb), true/false/null -> simple values.

Map keys are emitted in the order they appear in the description, never sorted: a
fixture's whole purpose is to pin what a decoder sees on the wire, including the
capitalisation and order of the field names. Lengths are always definite -- a fixture
that exercised indefinite-length encoding would say so by carrying different bytes.

Standard library only, Python 3.8+.
"""

from __future__ import annotations

import argparse
import base64
import json
import struct
import sys


def _head(major: int, value: int) -> bytes:
    """Major type byte plus argument, in the shortest form CBOR allows."""
    prefix = major << 5
    if value < 24:
        return bytes([prefix | value])
    if value < 0x100:
        return bytes([prefix | 24, value])
    if value < 0x10000:
        return bytes([prefix | 25]) + value.to_bytes(2, "big")
    if value < 0x100000000:
        return bytes([prefix | 26]) + value.to_bytes(4, "big")
    if value < 0x10000000000000000:
        return bytes([prefix | 27]) + value.to_bytes(8, "big")
    raise ValueError(f"argument {value} does not fit in a CBOR head")


def _double(value: float) -> bytes:
    """An IEEE 754 double (0xfb), the only float head this script emits.

    Always eight bytes, never a half or single: a fixture pins what a decoder sees,
    and the shortest-form rules of RFC 8949 §3.9 are a preference of an encoder rather
    than something a corpus should depend on.
    """
    return b"\xfb" + struct.pack(">d", value)


def encode(value) -> bytes:
    if value is None:
        return b"\xf6"
    if value is True:
        return b"\xf5"
    if value is False:
        return b"\xf4"
    if isinstance(value, int):
        if value >= 0:
            return _head(0, value)
        return _head(1, -1 - value)
    if isinstance(value, float):
        return _double(value)
    if isinstance(value, str):
        raw = value.encode("utf-8")
        return _head(3, len(raw)) + raw
    if isinstance(value, list):
        return _head(4, len(value)) + b"".join(encode(item) for item in value)
    if isinstance(value, dict):
        if set(value) == {"$bytes"}:
            raw = base64.b64decode(value["$bytes"], validate=True)
            return _head(2, len(raw)) + raw
        if set(value) == {"$tag"}:
            number, tagged = value["$tag"]
            return _head(6, number) + encode(tagged)
        if set(value) == {"$float"}:
            # float("inf") / float("nan") in the JSON description itself would not be
            # portable JSON, so the marker carries the name and this does the parsing.
            return _double(float(value["$float"]))
        if set(value) == {"$map"}:
            entries = value["$map"]
            out = _head(5, len(entries))
            for key, item in entries:
                out += encode(key) + encode(item)
            return out
        out = _head(5, len(value))
        for key, item in value.items():
            out += encode(key) + encode(item)
        return out
    raise TypeError(f"cannot encode {type(value).__name__} as CBOR")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("path", nargs="?", help="JSON description; stdin when omitted")
    parser.add_argument(
        "--prefix",
        type=int,
        default=None,
        metavar="N",
        help="emit only the first N bytes, for fixtures that record a truncated frame",
    )
    args = parser.parse_args()

    text = open(args.path, encoding="utf-8").read() if args.path else sys.stdin.read()
    encoded = encode(json.loads(text))
    if args.prefix is not None:
        encoded = encoded[: args.prefix]
    print(encoded.hex())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
