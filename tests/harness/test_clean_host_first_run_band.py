import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / "scripts/first-run-recovery.tsv"


def run(s):
    return subprocess.run(
        ["python3", str(ROOT / "scripts/clean-host-first-run-band.py")],
        input=s,
        text=True,
        capture_output=True,
    )


def test_real_fixture():
    assert run(P.read_text()).returncode == 0


def test_two_saved_refused():
    r = run(
        "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n"
        + "5\t1\t1\t1\t1\t1\t1\t10\t1\t10\t99\tagain verifier\n"
        "5\t1\t1\t1\t2\t1\t1\t30\t1\t10\t99\tkey\n"
        "5\t1\t1\t1\t3\t1\t1\t60\t1\t10\t99\tsaved offline\n"
        "5\t1\t1\t1\t4\t1\t1\t80\t1\t10\t99\tsaved offline\n"
    )
    assert r.returncode and "unique" in r.stderr


def test_again_only_refused():
    r = run(
        "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n"
        + "5\t1\t1\t1\t1\t1\t1\t10\t1\t10\t99\tagain\n"
        "5\t1\t1\t1\t2\t1\t1\t30\t1\t10\t99\tkey\n"
        "5\t1\t1\t1\t3\t1\t1\t60\t1\t10\t99\tsaved offline\n"
    )
    assert r.returncode and "identify" in r.stderr


def test_empty_band_refused():
    r = run(
        "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n"
        + "5\t1\t1\t1\t1\t1\t1\t50\t1\t10\t99\tagain verifier\n"
        "5\t1\t1\t1\t2\t1\t1\t50\t1\t10\t99\tsaved offline\n"
    )
    assert r.returncode and "empty" in r.stderr


def test_key_only_again_refused():
    r = run(
        "".join(
            [
                "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
                "5\t1\t1\t1\t1\t1\t1\t150\t1\t12\t99\tQRMY-AGAIN-AVD5\n",
                "5\t1\t1\t1\t2\t1\t1\t220\t1\t10\t99\tI saved offline\n",
            ]
        )
    )
    assert r.returncode and "identify" in r.stderr


def test_adjacent_anchors_refused():
    r = run(
        "".join(
            [
                "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
                "5\t1\t1\t1\t1\t1\t1\t10\t1\t10\t99\tverifier again\n",
                "5\t1\t1\t1\t2\t1\t1\t30\t1\t10\t99\tI saved offline\n",
            ]
        )
    )
    assert r.returncode and "contains no key line" in r.stderr
