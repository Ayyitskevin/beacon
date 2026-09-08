#!/usr/bin/env python3
import csv, sys
rows=list(csv.DictReader(sys.stdin, delimiter='\t')); lines={}
for r in rows: lines.setdefault((r['block_num'],r['par_num'],r['line_num']),[]).append(r)
def norm(s): return ''.join(c.casefold() for c in s if c.isalnum() or c.isspace())
def find(*tokens): return [rs for rs in lines.values() if all(t in norm(' '.join(r.get('text','') for r in rs)) for t in tokens)]
again=find('again','verifier'); saved=find('saved'); saved=[rs for rs in saved if ('offline' in norm(' '.join(r.get('text','') for r in rs)) or 'recovery' in norm(' '.join(r.get('text','') for r in rs)))]
if len(again)!=1 or len(saved)!=1: raise SystemExit('could not identify unique Recovery Key instruction and checkbox lines')
a,b=again[0],saved[0]; at=max(int(r['top'])+int(r['height']) for r in a); bt=min(int(r['top']) for r in b)
if at>=bt or not any(at < int(r['top']) and int(r['top'])+int(r['height']) < bt for rs in lines.values() for r in rs): raise SystemExit('Recovery Key redaction band is empty or contains no key line')
print(at+4,bt-4)
