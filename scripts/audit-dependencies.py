#!/usr/bin/env python3
"""Check every locked registry crate against OSV (includes RustSec advisories).

Only public package names and versions are sent. A failed lookup is not a pass.
Run npm audit separately for the website lockfile.
"""
import json
from pathlib import Path
import sys
import tomllib
import urllib.request

root = Path(__file__).resolve().parent.parent
with (root / "Cargo.lock").open("rb") as file:
    packages = [p for p in tomllib.load(file)["package"]
                if p.get("source", "").startswith("registry+")]
request = urllib.request.Request(
    "https://api.osv.dev/v1/querybatch",
    data=json.dumps({"queries": [{"package": {"name": p["name"], "ecosystem": "crates.io"},
                                    "version": p["version"]} for p in packages]}).encode(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(request, timeout=30) as response:
    results = json.load(response)["results"]
if len(results) != len(packages):
    raise RuntimeError("Incomplete advisory response")
findings = [(p, v["id"]) for p, r in zip(packages, results) for v in r.get("vulns", [])]
for package, advisory in findings:
    print(f'{package["name"]} {package["version"]}: {advisory}')
print(f"Checked {len(packages)} locked registry packages; {len(findings)} advisory matches")
sys.exit(1 if findings else 0)
