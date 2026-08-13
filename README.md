# Knight

Knight is a Rust build executor designed as a drop-in replacement for Ninja.
It is early in its compatibility work: the core manifest parser, incremental
graph planner, parallel executor, dependency logs, dyndeps, pools, response
files, inherited jobservers, load limiting, phase-level statistics, and common
CLI tools are implemented. Canonical path identity and both Ninja status-format
syntaxes are covered by differential tests. It is not yet feature-complete.

```powershell
cargo build --release
target\release\knight.exe -C path\to\build
```

Implemented syntax includes variables, rules, build edges, explicit and
implicit outputs, explicit/implicit/order-only/validation inputs, `default`,
`include`, scoped `subninja`, `pool`, `phony`, escaped paths, and line
continuations. GCC depfiles, MSVC `/showIncludes`, and Ninja's build/dependency
logs are supported and covered by upstream interoperability tests.

The differential benchmark runner expects an upstream `ninja` executable:

```powershell
powershell -NoProfile -File scripts\benchmark.ps1 -Ninja C:\path\to\ninja.exe
powershell -NoProfile -File scripts\benchmark-dyndep.ps1 -Ninja C:\path\to\ninja.exe
powershell -NoProfile -File scripts\benchmark-inputs.ps1 -Ninja C:\path\to\ninja.exe
powershell -NoProfile -File scripts\benchmark-status.ps1 -Ninja C:\path\to\ninja.exe
powershell -NoProfile -File scripts\benchmark-compdb.ps1 -Ninja C:\path\to\ninja.exe
powershell -NoProfile -File scripts\benchmark-phony.ps1 -Ninja C:\path\to\ninja.exe
powershell -NoProfile -File scripts\benchmark-pools.ps1 -Ninja C:\path\to\ninja.exe
wsl -e bash scripts/benchmark-status-pty.sh /path/to/ninja /path/to/knight
python scripts\differential-fuzz.py --ninja C:\path\to\ninja.exe --knight target\release\knight.exe --execute
python scripts\differential-fuzz.py --ninja C:\path\to\ninja.exe --knight target\release\knight.exe --missing-sources --execute
```

See [COMPATIBILITY.md](COMPATIBILITY.md) for the current audited surface.
