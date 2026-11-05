"""Replay the public UCI fixture commands against frozen public 0.1.0.

Run with python3 from any directory. Builds only in a temporary directory and
uses no network or net file. The position commands in the TSV remain fixed.
"""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
CRATE = "three_kingdoms_banqi_rust"
BASELINE = "d314fdbbd5ffe820e50a0be315b1259ec6b43da5"
FIXTURE = Path(__file__).with_name("uci-v010.tsv")


def run(*args, **kwargs):
    return subprocess.run(
        args, check=True, text=True, stdout=subprocess.PIPE, **kwargs
    ).stdout


commands = [
    line.split("\t")[0]
    for line in FIXTURE.read_text().splitlines()
    if line and not line.startswith("#")
]
assert len(commands) == 200
with tempfile.TemporaryDirectory(prefix="banqi-uci-010-") as directory:
    root = Path(directory)
    files = ["Cargo.toml", "Cargo.lock", f"{CRATE}/Cargo.toml"]
    files += run(
        "git", "ls-tree", "-r", "--name-only", BASELINE, "--", f"{CRATE}/src", cwd=ROOT
    ).splitlines()
    for name in files:
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(run("git", "show", f"{BASELINE}:{name}", cwd=ROOT))
    run(
        "cargo", "build", "--offline", "--release", "--jobs", "1",
        "--manifest-path", str(root / "Cargo.toml"),
        "--target-dir", str(root / "target"), "--bin", "three-kingdoms-engine",
    )
    script = "\n".join(f"{command}\ngo nodes 5000 movetime 0" for command in commands) + "\nquit\n"
    replies = run(str(root / "target/release/three-kingdoms-engine"), input=script).splitlines()
assert len(replies) == 2 * len(commands)
lines = [
    f"# v0.1.0 {BASELINE[:7]}; nodes=5000, movetime=0, default depth/q/TT; "
    "command<TAB>bestmove<TAB>score_cp<TAB>depth<TAB>nodes"
]
for command, info, best in zip(commands, replies[::2], replies[1::2]):
    fields = info.split()
    lines.append("\t".join([
        command, best.removeprefix("bestmove "),
        fields[fields.index("cp") + 1], fields[fields.index("depth") + 1],
        fields[fields.index("nodes") + 1],
    ]))
FIXTURE.write_text("\n".join(lines) + "\n")
print(f"Wrote {len(commands)} public 0.1.0 fixture positions")
