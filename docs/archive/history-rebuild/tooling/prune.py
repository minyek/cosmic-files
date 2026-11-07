#!/usr/bin/python3 -I
"""prune.py <worktree>: remove `use` names that do not resolve (E0432) or are unused at this stage, until clean.
Prints one line per pruned name. Exit 0 when no such diagnostics remain, 1 if cargo still fails (other errors)."""
import json, re, subprocess, sys, os
wt=sys.argv[1]
FLAGS=sys.argv[2:]   # extra cargo flags, e.g. --no-default-features
def check():
    p=subprocess.run(['cargo','check','--locked','--all-targets','--message-format=json']+FLAGS,cwd=wt,capture_output=True,text=True)
    msgs=[]
    for ln in p.stdout.split('\n'):
        if ln.startswith('{'):
            try: j=json.loads(ln)
            except Exception: continue
            if j.get('reason')=='compiler-message': msgs.append(j['message'])
    return p.returncode,msgs
def fmt_stmt(stmt,indent):
    depth=indent//4
    src=''.join('    '*i+'mod m%d {\n'%i for i in range(depth))+stmt.strip()+'\n'
    src='\n'.join(('    '*depth+l if l.strip() else l) for l in src.split('\n')) if False else src
    # place the statement at the right indent inside the modules
    lines=[('    '*depth+stmt.strip())] 
    src=''.join('    '*i+'mod m%d {\n'%i for i in range(depth))+lines[0]+'\n'+''.join('    '*(depth-1-i)+'}\n' for i in range(depth))
    r=subprocess.run(['rustfmt','--edition','2024','--emit','stdout'],input=src,capture_output=True,text=True)
    if r.returncode!=0: return None
    out=r.stdout.split('\n')
    body=out[depth:len(out)-1-depth-0]
    body=[l for l in out[depth:] ][:len(out)-depth]
    # strip closing braces
    res=out[depth:]
    # remove trailing closers
    k=len(res)
    while k>0 and (res[k-1].strip()=='' or res[k-1].strip()=='}'): k-=1
    return '\n'.join(res[:k])
def span_edits(msgs):
    ed={}  # file -> list of (start,end)
    why={}
    for m in msgs:
        code=(m.get('code') or {}).get('code')
        txt=m['message']
        if code=='E0432' or (m['level']=='warning' and (m.get('code') or {}).get('code')=='unused_imports'):
            for s in m['spans']:
                if s['is_primary'] and s['file_name'].startswith(('src','tests','examples')):
                    ed.setdefault(s['file_name'],set()).add((s['byte_start'],s['byte_end']))
                    why[(s['file_name'],s['byte_start'],s['byte_end'])]=txt[:60]
    return ed,why
def rewrite(path,spans):
    full=os.path.join(wt,path)
    data=open(full,'rb').read()
    # locate statements
    text=data
    stmts={}
    for (a,b) in spans:
        # statement start: scan back to line start containing 'use ' keyword at statement beginning
        i=text.rfind(b'\nuse ',0,a); j=text.rfind(b'\n    use ',0,a); k=text.rfind(b'\n        use ',0,a); l=text.rfind(b'\npub use ',0,a); m2=text.rfind(b'\n    pub use ',0,a); n=text.rfind(b'\n    #[cfg',0,a)
        start=max(i,j,k,l,m2)
        if start<0: print('no statement for',path,a); continue
        end=text.find(b';',b)
        # ensure the statement ends at the first ';' after the span (use trees contain no ';')
        stmts.setdefault((start+1,end+1),[]).append((a,b))
    out=text
    for (s,e),sp in sorted(stmts.items(),reverse=True):
        stmt=text[s:e].decode()
        indent=len(stmt)-len(stmt.lstrip(' '))
        rel=sorted([(a-s,b-s) for a,b in sp],reverse=True)
        sb=text[s:e]
        for a,b in rel: sb=sb[:a]+sb[b:]
        t=sb.decode()
        # clean commas and empty groups
        for _ in range(8):
            t=re.sub(r',\s*,',',',t); t=re.sub(r'\{\s*,',r'{',t); t=re.sub(r',\s*\}',r'}',t)
            t=re.sub(r'[\w:]+::\{\s*\}','',t)
            t=re.sub(r'\{\s*([\w:]+)\s*\}',r'{\1}',t) if False else t
        t=t.strip()
        removed=re.fullmatch(r'(pub\s+)?use\s*;?',t.replace('{}','')) or re.fullmatch(r'(pub\s+)?use\s+\{?\}?;',t) or t in('use ;','use;')
        # a single-name group `a::{b}` stays as is; rustfmt flattens braces? it does not, accept
        if removed or re.fullmatch(r'(pub\s+)?use\s+[\w:]*(::)?;',t) and t.endswith('::;'):
            new=''
        else:
            f=fmt_stmt(t,indent)
            new=(f if f is not None else (' '*indent+t.lstrip()))+'\n'
        out=out[:s]+new.encode()+out[e+(1 if out[e:e+1]==b'\n' else 0):]
    open(full,'wb').write(out)
def flatten(tree):
    """'a::{b, c::{d, e as f}, g}' -> [(path-without-name, name, alias)]"""
    tree=tree.strip()
    out=[]
    def rec(prefix,t):
        t=t.strip()
        if not t: return
        # split on top-level commas
        parts=[];d=0;cur=''
        for ch in t:
            if ch=='{': d+=1
            if ch=='}': d-=1
            if ch==',' and d==0: parts.append(cur);cur=''
            else: cur+=ch
        if cur.strip(): parts.append(cur)
        for p in parts:
            p=p.strip()
            if not p: continue
            m=re.match(r'^([\w:]*?)::\{(.*)\}$',p,re.S) or re.match(r'^()\{(.*)\}$',p,re.S)
            if m: rec(prefix+(m.group(1)+'::' if m.group(1) else ''),m.group(2))
            else: out.append((prefix.rstrip(':') if prefix else '',p.split(' as ')[0].split('::')[-1] if ' as ' not in p else p,p))
    rec('',tree)
    return out
def base_imports(path):
    r=subprocess.run(['git','-C',wt,'show','e03001c:'+path],capture_output=True,text=True)
    if r.returncode: return {}
    res={}
    for m in re.finditer(r'^[ ]*(?:pub(?:\([\w ]+\))? )?use ([^;]+);',r.stdout,re.M):
        for prefix,name,full in flatten(m.group(1)):
            full_path=(prefix+'::' if prefix else '')+full.split(' as ')[0].split('::')[-1] if prefix else full
            res.setdefault(full.split(' as ')[-1].split('::')[-1],[]).append((prefix,full))
    return res
def restore(msgs):
    """Re-add base imports that a pruned/applied final list dropped but the staged code still uses."""
    need={}
    for m in msgs:
        code=(m.get('code') or {}).get('code')
        if m['level']=='error' and code in ('E0425','E0412','E0433','E0599','E0422','E0423','E0405','E0574'):
            for s in m['spans']:
                if s['is_primary'] and s['file_name'].startswith(('src','tests','examples')):
                    syms=re.findall(r'`([A-Za-z_]\w*)`',m['message'])
                    for sym in syms: need.setdefault(s['file_name'],set()).add((sym,s['byte_start']))
    n=0
    for f,syms in need.items():
        bi=base_imports(f)
        full=os.path.join(wt,f); text=open(full).read()
        tm=re.search(r'\n#\[cfg\(test\)\]\nmod tests\b',text)
        for sym,off in sorted(syms):
            if sym not in bi: continue
            prefix,fullname=bi[sym][0]
            if tm and len(text[:tm.start()].encode())<=off: region=text[tm.start():]
            else: region=text[:tm.start()] if tm else text
            if re.search(r'^[ ]*(pub )?use [^;]*\b'+re.escape(sym)+r'\b[^;]*;',region,re.M): continue
            if prefix and re.search(r'(^|[^\w])'+re.escape(prefix)+r'::\{',region):
                a=(tm.start() if tm and region is text[tm.start():] else 0)
                seg=re.sub(re.escape(prefix)+r'::\{',prefix+'::{'+fullname+', ',region,count=1)
                text=(text[:tm.start()]+seg) if tm and region is text[tm.start():] else (seg+text[tm.start():] if tm else seg)
            else:
                # add a statement after the last top-level use
                ms=list(re.finditer(r'^use [^;]+;\n',text,re.M))
                stmt='use '+((prefix+'::') if prefix else '')+fullname+';\n'
                if stmt in text:
                    continue  # already restored once; adding it again cannot help
                pos=ms[-1].end() if ms else 0
                text=text[:pos]+stmt+text[pos:]
            print('restore',f,sym); n+=1
        open(full,'w').write(text)
    # reformat touched top-level use statements via rustfmt on whole file is avoided: format each changed statement
    return n
for rnd in range(30):
    rc,msgs=check()
    ed,why=span_edits(msgs)
    if not ed:
        if rc!=0 and restore(msgs): continue
        sys.exit(0 if rc==0 else 1)
    for f,sp in ed.items():
        for a,b in sorted(sp): print('prune',f,why[(f,a,b)])
        rewrite(f,sp)
sys.exit(2)
