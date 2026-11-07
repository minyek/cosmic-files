#!/usr/bin/python3 -I
"""lint-order.py <order-file> [start] [end]: for each cumulative stage: stage, prune, fmt-stage, then cargo check in three
configs and clippy (default). Reports findings not in the base/target baselines. Logs to commits/lint-summary.txt."""
import subprocess,sys,importlib.util
B='/mnt/logs/temp/history-rebuild/bin'; OUT='/mnt/logs/temp/history-rebuild/commits'
WT='/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-series'
spec=importlib.util.spec_from_file_location('cc',B+'/check-lib.py'); cc=importlib.util.module_from_spec(spec); spec.loader.exec_module(cc)
order=[l.strip() for l in open(sys.argv[1]) if l.strip()]; start=int(sys.argv[2]) if len(sys.argv)>2 else 1
end=int(sys.argv[3]) if len(sys.argv)>3 else len(order)
def run(c): return subprocess.run(c,cwd=WT,capture_output=True,text=True)
summ=open(OUT+'/lint-summary.txt','a'); summ.write(f'--- {sys.argv[1]} from {start}\n')
done=[]
for i,unit in enumerate(order,1):
    done+=unit.split(',')
    if i<start: continue
    if i>end: break
    run(['git','checkout','-q','--','.']); run(['git','clean','-fdq'])
    r=run([B+'/stage.py',WT,','.join(done)])
    if r.returncode: summ.write(f'{i} {unit} STAGE-FAIL {r.stderr[-200:]}\n'); sys.exit(2)
    run([B+'/prune.py',WT])
    r=run([B+'/fmt-stage.py',WT])
    bad=[]
    if r.returncode: bad.append('fmt-stage: '+r.stderr.strip()[-200:])
    for cfg,flags in cc.CFG.items():
        r=run(['cargo','check','--locked','--all-targets','--message-format=short']+flags)
        if r.returncode: bad.append(f'check-{cfg} rc={r.returncode}')
        bad+=[f'check-{cfg}: {x[0]}: {x[2]}' for x in sorted(cc.findings(r.stdout+r.stderr)-cc.BASE[('check',cfg)])]
    r=run(['cargo','clippy','--locked','--all-targets','--keep-going','--message-format=short'])
    bad+=[f'clippy: {x[0]}: {x[2]}' for x in sorted(cc.findings(r.stdout+r.stderr)-cc.BASE[('clippy','default')])]
    summ.write(f"{i:02d} {unit} {'OK' if not bad else 'BAD'}\n"+''.join(f'    {b}\n' for b in bad)); summ.flush()
run(['git','checkout','-q','--','.']); run(['git','clean','-fdq'])
