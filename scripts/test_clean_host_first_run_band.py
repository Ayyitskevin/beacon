import subprocess
from pathlib import Path
P=Path(__file__).with_name('first-run-recovery.tsv')
def run(tsv): return subprocess.run(['python3',str(Path(__file__).with_name('clean-host-first-run-band.py'))],input=tsv,text=True,capture_output=True)
def test_real_fixture(): assert run(P.read_text()).returncode==0
def row(line,text,top,height=10): return f'5\t1\t1\t1\t{line}\t1\t1\t{top}\t1\t{height}\t99\t{text}\n'
def test_two_saved_refused(): assert run(row(1,'again',10)+row(2,'saved',50)+row(3,'saved',70)).returncode!=0
def test_again_only_refused(): assert run(row(1,'again',10)).returncode!=0
def test_empty_band_refused(): assert run(row(1,'again',50)+row(2,'saved',50)).returncode!=0
