"""Export log callsites with their complete localized templates for copy review.

This is a source catalogue, not proof of executing a device workflow. Dynamic
branches and values remain visible in `call`; translations are never flattened
into an invented successful transcript. Run from any directory:
  python packaging/log_catalog.py --output ../audits/log-catalog.json
"""
import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CALL = re.compile(r"\b(?:live!|live_debug!|log_push|log\.push|live_sink::progress)\s*\(")
TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{")


def call_end(text, start):
    depth = 1
    quoted = False
    escaped = False
    i = start
    while i < len(text):
        char = text[i]
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif text.startswith("//", i):
            i = text.find("\n", i)
            if i < 0:
                return len(text)
        elif char == '"':
            quoted = True
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    raise ValueError("unterminated log call")


def catalogue():
    locales = {p.stem: json.loads(p.read_text(encoding="utf-8"))
               for p in (ROOT / "crates/ltbox-gui/lang").glob("*.json")}
    records = []
    for path in sorted((ROOT / "crates").glob("*/src/**/*.rs")):
        if path.stem.endswith("_tests") or path.stem == "demo":
            continue
        text = path.read_text(encoding="utf-8")
        test = TEST_MODULE.search(text)
        if test:
            text = text[:test.start()]
        for match in CALL.finditer(text):
            # Documentation examples and comments are not executable callsites.
            line_start = text.rfind("\n", 0, match.start()) + 1
            if text[line_start:match.start()].lstrip().startswith("//"):
                continue
            end = call_end(text, match.end())
            call = text[match.start():end]
            keys = sorted({k for k in re.findall(r'"([a-z][a-z0-9_]+)"', call)
                           if k in locales["en"]})
            records.append({
                "file": path.relative_to(ROOT).as_posix(),
                "line": text[:match.start()].count("\n") + 1,
                "call": " ".join(call.split()),
                "templates": {k: {lang: table.get(k) for lang, table in locales.items()}
                              for k in keys},
            })
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    records = catalogue()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(records, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{len(records)} callsites in {len({r['file'] for r in records})} files -> {args.output}")


if __name__ == "__main__":
    main()
