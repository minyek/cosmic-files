#!/usr/bin/python3 -I
"""build-series.py <order-file>: commit one stage per unit on history-rebuild-series (must be at e03001c, clean).
Messages: subject from map/subjects.tsv; body from each issue's Cause/Fix/Tests; trailers per D11."""
import re,subprocess,sys,os
WT='/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-series'
B='/mnt/logs/temp/history-rebuild/bin'; M='/mnt/logs/temp/history-rebuild/map'
subj=dict(l.rstrip('\n').split('\t',1) for l in open(M+'/subjects.tsv'))
def sh(*a,inp=None):
    r=subprocess.run(a,cwd=WT,capture_output=True,text=True,input=inp)
    if r.returncode: print('FAIL',a[:3],r.stderr[-300:],flush=True); sys.exit(3)
    return r.stdout
def sections(n):
    t=open(f'{M}/issues/{n}.md').read()
    out={}
    for m in re.finditer(r'^## (.+?)\n(.*?)(?=^## |\Z)',t,re.S|re.M): out[m.group(1).strip()]=m.group(2).strip()
    return out
def fix(txt): return re.sub(r'(?<![\w/])#(\d{1,3})\b',r'minyek/cosmic-files#\1',txt)
def body(unit):
    ns=unit.split(',')
    parts=[]
    for n in ns:
        s=sections(n)
        head=open(f'{M}/issues/{n}.md').read().split('\n')[0]
        title=re.sub(r'^# #\d+ ','',head); title=re.sub(r' \[(CLOSED|OPEN)\]$','',title)
        blk=[]
        if len(ns)>1: blk.append(f'minyek/cosmic-files#{n}: {title}')
        for k in ('Problem','Cause','Fix','Tests'):
            if k in s and s[k] and not s[k].startswith('None'):
                text='\n'.join(l for l in fix(s[k]).split('\n') if not l.startswith(('Landed in', 'Found during the history rebuild'))).strip().replace('\n\n','\n')
                blk.append(f'{k}:\n{text}' if re.match(r'(- |\d+\. )',text) else f'{k}: {text}')
        parts.append('\n'.join(blk))
    return '\n\n'.join(parts)
order=[l.strip() for l in open(sys.argv[1]) if l.strip()]
done=[]
K=int(sh('git','rev-list','--count','e03001c..HEAD').strip())
for idx,unit in enumerate(order):
    done+=unit.split(',')
    if idx<K: continue
    sh('git','checkout','-q','--','.'); sh('git','clean','-fdq')
    sh(B+'/stage.py',WT,','.join(done))
    r=subprocess.run([B+'/prune.py',WT],cwd=WT,capture_output=True,text=True)
    if r.returncode: print('prune rc',r.returncode,unit,flush=True); sys.exit(4)
    sh(B+'/fmt-stage.py',WT)
    st=sh('git','status','--porcelain=v1','-uall')
    paths=[l[3:] for l in st.split('\n') if l]
    sh('git','add','--',*paths)
    trailers='\n'.join(f'Fork-issue: minyek/cosmic-files#{n}' for n in unit.split(','))+'\nAssisted-by: Claude'
    msg=f'{subj[unit]}\n\n{body(unit)}\n\n{trailers}\n'
    sh('git','commit','-q','-F','-',inp=msg)
    print('committed',unit,sh('git','rev-parse','--short','HEAD').strip(),flush=True)
print('DONE')
