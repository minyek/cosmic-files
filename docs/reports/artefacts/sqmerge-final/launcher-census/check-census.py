from pathlib import Path
import subprocess, tempfile, sys
function=Path(sys.argv[1]).read_text()
cases=[('empty-root',[],0),('empty-nested-slices',[('jobs-arm.slice/jobs-arm-policy.slice',0)],0),('live-nested-job',[('jobs-arm.slice/jobs-arm-policy.slice/arm-test.service',1)],1),('exited-job',[('jobs-batch.slice/arm-ended.service',0)],0),('two-compartments',[('a/jobs-batch.slice/arm-one.service',1),('b/jobs-batch.slice/arm-two.scope',1)],2)]
failures=0
for name, entries, expected in cases:
 with tempfile.TemporaryDirectory(dir='/mnt/logs/temp/cosmic-arm-census') as temp:
  root=Path(temp)
  for relative,populated in entries:
   p=root/relative;p.mkdir(parents=True,exist_ok=True)
   (p/'cgroup.events').write_text(f'populated {populated}\nfrozen 0\n')
  compartments=[root/'a',root/'b'] if name=='two-compartments' else [root]
  shell=function+"\njobs_compartments() { printf '%s\\n' "+' '.join("'"+str(p)+"'" for p in compartments)+"; }\nlive_job_count\n"
  result=subprocess.run(['bash','-c',shell],capture_output=True,text=True,check=True)
  actual=int(result.stdout.strip())
  print(f'{name}: expected={expected} actual={actual} '+('PASS' if actual==expected else 'FAIL'))
  failures+=actual!=expected
sys.exit(bool(failures))
