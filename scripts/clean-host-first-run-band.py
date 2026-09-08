#!/usr/bin/env python3
import csv, sys
rows=list(csv.DictReader(sys.stdin, delimiter='\t'))
lines={}
for r in rows:
    key=(r.get('block_num'),r.get('par_num'),r.get('line_num'))
    lines.setdefault(key,[]).append(r)
def norm(s):
    return ''.join(c.casefold() for c in s if c.isalnum() or c.isspace())
def find(token): return [rs for rs in lines.values() if token in norm(' '.join(r.get('text','') for r in rs))]
again,saved=find('again'),find('saved')
if len(again)!=1 or len(saved)!=1: raise SystemExit('could not bound the Recovery Key between its instruction and checkbox')
top=max(int(r['top'])+int(r['height']) for r in again[0])+4; bottom=min(int(r['top']) for r in saved[0])-4
if top>=bottom: raise SystemExit('Recovery Key redaction band is empty')
print(top,bottom)
