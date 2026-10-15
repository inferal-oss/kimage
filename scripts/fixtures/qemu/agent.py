#!/usr/bin/python3
# Fixture provider only. Bounded, predefined diagnostics and cache-eviction actions.
import json,pathlib,re,subprocess,time,os
os.environ["KUBECONFIG"]="/mnt/output/config"
share=pathlib.Path('/mnt/output')
cluster=pathlib.Path('/mnt/prereqs/cluster.json')
if cluster.exists():
    import importlib.util
    spec=importlib.util.spec_from_file_location('owned_cluster_network','/mnt/prereqs/cluster-network.py')
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    module.configure(restart=True)
# Provisioning compatibility for the task-owned fixture's first service revision.
# Restricted SLIRP needs this private route before Kubernetes Service DNAT. Keep
# it persistent across fixture reboots; hypervisor restrict=on still blocks egress.
service=pathlib.Path('/etc/systemd/system/kimage-fixture.service')
unit=service.read_text()
assert 'Description=Task-owned isolated Kubernetes fixture' in unit
assert cluster.exists() or '--node-name kimage-clean --node-ip 10.0.2.15 --flannel-iface enp0s2' in unit
if not cluster.exists():
    route='ExecStartPre=/usr/sbin/ip route replace default via 10.0.2.2 dev enp0s2\n'
    if route not in unit:
        service.write_text(unit.replace('Type=simple\n','Type=simple\n'+route))
        subprocess.run(['systemctl','daemon-reload'],check=True,timeout=15)
    subprocess.run(['/usr/sbin/ip','route','replace','default','via','10.0.2.2','dev','enp0s2'],check=True,timeout=5)
seen=None
while True:
    request=share/'request.json'
    if request.exists():
        try:
            r=json.loads(request.read_text())
            if r['id']!=seen:
                seen=r['id']; op=r['operation']
                if op=='status':
                    cmds=[['systemctl','is-active','kimage-fixture'],['/usr/local/bin/k3s','kubectl','get','nodes','-o','wide'],['ip','route']]
                    output=[]
                    for cmd in cmds:
                        p=subprocess.run(cmd,timeout=15,capture_output=True,text=True)
                        output.append(dict(command=cmd,code=p.returncode,stdout=p.stdout[:5000],stderr=p.stderr[:1000]))
                    logs=subprocess.run(['journalctl','-u','kimage-fixture','-n','100','--no-pager','-o','cat'],capture_output=True,text=True).stdout
                    output.append(dict(errors=[s[:500] for s in logs.splitlines() if ('error' in s.lower() or 'fatal' in s.lower()) and 'token' not in s.lower()]))
                elif op=='evict_system_images':
                    names=r['images'];assert len(names)<=16
                    assert all(re.fullmatch(r'127\.0\.0\.1:17641/kimage-system/system-[a-z]+@sha256:[0-9a-f]{64}',n) for n in names)
                    p=subprocess.run(['/usr/local/bin/k3s','crictl','rmi']+names,timeout=90,capture_output=True,text=True)
                    output=dict(code=p.returncode,stdout=p.stdout[:3000],stderr=p.stderr[:3000])
                elif op=='evict_retained_images':
                    names=r['images'];assert 1<=len(names)<=16
                    assert all(re.fullmatch(r'127\.0\.0\.1:17641/kimage-app/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}@sha256:[0-9a-f]{64}',n) for n in names)
                    p=subprocess.run(['/usr/local/bin/k3s','crictl','rmi']+names,timeout=90,capture_output=True,text=True)
                    output=dict(code=p.returncode,stdout=p.stdout[:3000],stderr=p.stderr[:3000])
                elif op=='images':
                    p=subprocess.run(['/usr/local/bin/k3s','crictl','images','-o','json'],timeout=15,capture_output=True,text=True)
                    output=json.loads(p.stdout)
                elif op=='health':
                    # Provider diagnostics only; no arbitrary guest command input.
                    output=[]
                    commands=[['systemctl','show','kimage-fixture','-p','NRestarts','-p','ExecMainStatus','-p','ExecMainPID','-p','ActiveEnterTimestamp','-p','MemoryCurrent'],['cat','/proc/meminfo'],['cat','/proc/loadavg'],['df','-P','/'],['journalctl','-k','-n','300','--no-pager','-o','cat']]
                    for command in commands:
                        try:
                            value=subprocess.run(command,capture_output=True,text=True,timeout=10)
                            stdout=value.stdout
                            if command[0]=='journalctl':
                                stdout='\n'.join(line[:500] for line in stdout.splitlines() if re.search(r'oom|killed process|out of memory|watchdog|hung task',line,re.I))
                            output.append(dict(command=command,code=value.returncode,stdout=stdout[:6000]))
                        except subprocess.TimeoutExpired:
                            output.append(dict(command=command,timeout=True))
                    value=subprocess.run(['journalctl','-u','kimage-fixture','-n','2000','--no-pager','-o','cat'],capture_output=True,text=True,timeout=10)
                    lines=[line[:700] for line in value.stdout.splitlines() if re.search(r'oom|killed|failed with result|main process exited|panic|fatal|leader.*lost|^F[0-9]{4}|error retrieving resource lock',line,re.I) and not re.search(r'token|password|secret|authorization',line,re.I)]
                    output.append(dict(serviceFailures=lines[-40:]))
                elif op=='external_probe':
                    code = """import urllib.request,urllib.error
try:
    urllib.request.urlopen('https://registry-1.docker.io/v2/',timeout=3)
    print('reachable')
except urllib.error.HTTPError:
    print('reachable')
except Exception:
    print('unreachable')
"""
                    try:
                        p=subprocess.run(['/usr/bin/python3','-c',code],timeout=5,capture_output=True,text=True)
                        output=dict(externalReachable=p.stdout.strip()=='reachable')
                    except subprocess.TimeoutExpired:output=dict(externalReachable=False,deadlineReached=True)
                else: raise ValueError('unsupported fixture action')
                target=share/'response.json';pending=share/'response.pending'
                pending.write_text(json.dumps(dict(id=seen,output=output)));pending.replace(target)
        except Exception as e:
            (share/'response.json').write_text(json.dumps(dict(id=seen,error=type(e).__name__)))
    time.sleep(1)
