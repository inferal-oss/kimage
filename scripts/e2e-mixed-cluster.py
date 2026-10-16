#!/usr/bin/env python3
"""Opt-in real mixed-cluster E2E. Never installs kimage or changes build policy.

Published fixture Images, inputs, demands, and releases remain in the dedicated
application namespace. Only this run's consumer Deployments are removed.
"""
import argparse
import base64
import copy
import json
from pathlib import Path
import re
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
ARCHITECTURES = {'amd64', 'arm64'}
RUN_LABEL = 'kimage.dev/mixed-e2e'
PARENT_LABEL = 'kimage.dev/platform-parent'
INDEX = 'application/vnd.oci.image.index.v1+json'
MANIFEST = 'application/vnd.oci.image.manifest.v1+json'


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def is_ready(resource):
    status = resource.get('status') or {}
    return status.get('observedGeneration') == resource['metadata'].get('generation', 1) and any(
        c['type'] == 'Ready' and c['status'] == 'True' for c in status.get('conditions', []))


def platform_set(values):
    require(all(p.get('os') == 'linux' and p.get('architecture') in ARCHITECTURES for p in values), 'Unsupported test platform')
    return {p['architecture'] for p in values}


def assert_variants(image, children, releases, demand, expected):
    """Inspect identities and actual publication, not just parent Ready status."""
    uid = image['metadata']['uid']
    require(is_ready(image), 'Image is not Ready for its current generation')
    require(demand['spec']['imageUid'] == uid and demand['spec']['generation'] == image['metadata']['generation'], 'Stale or foreign platform demand')
    require(platform_set(demand['spec']['platforms']) == expected, 'Demand requested unexpected platforms')
    require(len(children) == len(expected), 'Unexpected number of native child Images')
    require(platform_set([c['spec']['platform'] for c in children]) == expected, 'Wrong native child platforms')
    require(set(image['status']['platformReleases']) == {'linux/'+a for a in expected}, 'Unexpected native release references')
    for child in children:
        require(child['metadata']['labels'].get(PARENT_LABEL) == uid and any(
            owner['uid'] == uid and owner['kind'] == 'Image' for owner in child['metadata'].get('ownerReferences', [])), 'Foreign native child')
        require(is_ready(child), 'Native child has not completed')
    own = [r for r in releases if r['spec']['imageUid'] == uid]
    require(len(own) == len(expected) + (len(expected) == 2), 'Unexpected parent ImageRelease count')
    by_name = {r['metadata']['name']: r for r in releases}
    for arch in expected:
        native = by_name[image['status']['platformReleases']['linux/'+arch]]
        require(native['spec']['imageUid'] == uid and native['spec']['platform'] == {'os':'linux', 'architecture':arch}, 'Foreign native release')
        execution = native['spec']['provenance']['execution']
        require(execution['platform'] == {'os':'linux', 'architecture':arch} and execution['executedSteps'] > 0, 'Native RUN did not execute')
        child = next(c for c in children if c['spec']['platform']['architecture'] == arch)
        child_release = by_name[child['status']['releaseRef']]
        require(child_release['spec']['imageUid'] == child['metadata']['uid'] and child_release['spec']['root'] == native['spec']['root'], 'Native child publication was reused from another Image')
    for release in own:
        require(release['status']['verified'] and release['status']['observedGeneration'] == release['metadata'].get('generation', 1), 'Unverified parent release')
    selected = by_name[image['status']['releaseRef']]
    require(selected['spec']['imageUid'] == uid, 'Foreign selected release')
    require(selected['spec']['root']['mediaType'] == (INDEX if len(expected) == 2 else MANIFEST), 'Wrong selected OCI root type')
    require(selected['spec']['platform']['architecture'] == ('auto' if len(expected) == 2 else next(iter(expected))), 'Wrong selected release platform')
    return selected


def image_body(name, source):
    return dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=name), spec=dict(
        version='1.0.0', source=dict(build=dict(format='Containerfile',
            execution=dict(strategy='Direct', builderRef='default'),
            definition=dict(inline='FROM scratch\nARG TARGETARCH\nCOPY probe-${TARGETARCH} /probe\nCOPY case.txt /case.txt\nRUN ["/probe", "build"]\nENTRYPOINT ["/probe"]\n'),
            inputs=dict(contextPath='/workspace/context', volumes=[dict(name='source', configMap=dict(name=source, defaultMode=365))],
                volumeMounts=[dict(name='source', mountPath='/workspace/context', readOnly=True)])))))


def deployment_body(name, namespace, architecture=None):
    labels = {'kimage.dev/mixed-e2e-case': name}
    spec = dict(containers=[dict(name='app', image='platform://'+namespace+'/'+name+':1.0.0',
        resources=dict(requests=dict(cpu='5m', memory='8Mi'), limits=dict(cpu='100m', memory='32Mi')))],
        terminationGracePeriodSeconds=1, automountServiceAccountToken=False)
    if architecture:
        require(architecture in ARCHITECTURES, 'Unsupported placement')
        spec['nodeSelector'] = {'kubernetes.io/arch':architecture}
    else:
        # Each Pod can run on either architecture. Pod anti-affinity spreads the
        # two index-pinned consumers across architectures without selecting an
        # architecture for either Pod at admission time.
        spec['affinity'] = dict(podAntiAffinity=dict(requiredDuringSchedulingIgnoredDuringExecution=[
            dict(labelSelector=dict(matchLabels=labels), topologyKey='kubernetes.io/arch')]))
    return dict(apiVersion='apps/v1', kind='Deployment', metadata=dict(name=name), spec=dict(
        replicas=1 if architecture else 2, selector=dict(matchLabels=labels),
        template=dict(metadata=dict(labels=labels), spec=spec)))


def build_probes(directory):
    directory.mkdir(parents=True, exist_ok=True)
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    host = next(line.split(': ', 1)[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines() if line.startswith('host: '))
    linker = sysroot/'lib/rustlib'/host/'bin/rust-lld'
    require(linker.is_file(), 'Rust self-contained linker not found')
    for arch, target in [('amd64','x86_64-unknown-linux-musl'), ('arm64','aarch64-unknown-linux-musl')]:
        subprocess.run(['rustc', '--edition=2024', '--target', target, '-C', 'linker='+str(linker),
            '-C', 'link-self-contained=yes', '-C', 'opt-level=z', '-C', 'lto=yes', '-C', 'strip=symbols', '-C', 'panic=abort',
            str(ROOT/'scripts/fixtures/mixed-platform-probe.rs'), '-o', str(directory/('probe-'+arch))], check=True)
        verify_probe(directory/('probe-'+arch), arch)


def verify_probe(path, arch):
    data = path.read_bytes()
    require(len(data) >= 64 and data[:6] == b'\x7fELF\x02\x01' and int.from_bytes(data[18:20], 'little') == {'amd64':62,'arm64':183}[arch], 'Wrong probe ELF architecture')
    offset, width, count = (int.from_bytes(data[a:b], 'little') for a,b in [(32,40),(54,56),(56,58)])
    require(width >= 56 and offset+width*count <= len(data), 'Invalid probe ELF headers')
    require(all(int.from_bytes(data[offset+i*width:offset+i*width+4], 'little') != 3 for i in range(count)), 'Probe must be statically linked')
    return data


class MixedCluster:
    def __init__(self, config):
        required = {'kubeconfig','context','namespace','systemNamespace','installation','probes','output'}
        require(required <= config.keys() and not config.keys()-required-{'timeoutSeconds','settleSeconds'}, 'Invalid mixed-cluster configuration keys')
        for key in ('namespace','systemNamespace','installation'):
            require(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', config[key]), 'Invalid '+key)
        require(config['namespace'] != config['systemNamespace'] and not config['namespace'].startswith('kube-'), 'Dedicated application namespace required')
        require(isinstance(config['context'],str) and config['context'].strip(), 'Explicit kubeconfig context required')
        self.config = config
        self.command = ['kubectl','--kubeconfig',str(Path(config['kubeconfig']).expanduser().resolve()),
            '--context',config['context'],'--request-timeout=20s']
        self.namespace, self.system = config['namespace'], config['systemNamespace']
        self.run_id = uuid.uuid4().hex[:12]
        self.prefix = 'mixed-e2e-'+self.run_id
        self.output = Path(config['output']).expanduser().resolve()
        self.output.mkdir(parents=True, exist_ok=False)
        self.deployments, self.cases = [], []
        self.report = dict(schema=1, test='mixed-cluster', runId=self.run_id, context=config['context'], namespace=self.namespace,
            cases=[], complete=False, cleanupComplete=False)

    def call(self, arguments, value=None, ok=True):
        result = subprocess.run(self.command+arguments, input=None if value is None else json.dumps(value), text=True,
            capture_output=True, timeout=45)
        if ok and result.returncode:
            raise RuntimeError('kubectl '+str(arguments[:3])+': '+result.stderr[-2000:])
        return result

    def get(self, kind, name=None, namespace=None, selector=None):
        arguments = ['get',kind]+([name] if name else [])+(['-n',namespace] if namespace else [])+['-o','json']
        if selector:
            arguments += ['-l',selector]
        for attempt in range(3):
            result = self.call(arguments, ok=False)
            if result.returncode == 0:
                return json.loads(result.stdout)
            if 'NotFound' in result.stderr:
                return None
            if attempt == 2 or not any(s in result.stderr for s in ('INTERNAL_ERROR','EOF','timeout','connection reset','ServiceUnavailable')):
                raise RuntimeError(result.stderr[-2000:])
            time.sleep(1)

    def create(self, body):
        body = copy.deepcopy(body)
        body['metadata']['namespace'] = self.namespace
        body['metadata'].setdefault('labels', {})[RUN_LABEL] = self.run_id
        if body['kind'] == 'Deployment':
            body['spec']['template']['metadata']['labels'][RUN_LABEL] = self.run_id
        result = json.loads(self.call(['create','-f','-','-o','json'], body).stdout)
        if body['kind'] == 'Deployment':
            self.deployments.append((result['metadata']['name'],result['metadata']['uid']))
        return result

    def wait(self, function, description):
        deadline = time.monotonic()+self.config.get('timeoutSeconds',600)
        progress = time.monotonic()+30
        while time.monotonic() < deadline:
            result = function()
            if result:
                return result
            if time.monotonic() >= progress:
                print('WAIT: '+description,flush=True)
                progress = time.monotonic()+30
            time.sleep(2)
        raise RuntimeError('Timed out: '+description)

    def preflight(self):
        nodes = self.get('nodes')['items']
        self.nodes = {n['metadata']['name']:n for n in nodes}
        ready = {n['status']['nodeInfo']['architecture'] for n in nodes if n['status']['nodeInfo']['operatingSystem']=='linux'
            and not n.get('spec',{}).get('unschedulable') and any(c['type']=='Ready' and c['status']=='True' for c in n['status']['conditions'])}
        require(ARCHITECTURES <= ready, 'Requires real Ready Linux amd64 AND arm64 Nodes')
        config = self.get('configmap',self.config['installation']+'-config',self.system)
        require(config and config['metadata'].get('labels',{}).get('kimage.dev/installation')==self.config['installation'], 'Wrong installation configuration')
        policy = json.loads(config['data']['configuration.json'])
        require(policy.get('schema')==1 and policy.get('buildPolicy','OnDemand')=='OnDemand', 'Test requires existing OnDemand policy; it never changes administrator policy')
        self.configuration = config['data']['configuration.json']
        require(self.get('namespace',self.namespace) is not None, 'Application namespace must already exist and be managed by this installation')
        for arch in sorted(ARCHITECTURES):
            worker = self.get('pod',self.config['installation']+'-builder-'+arch,self.system)
            require(worker and any(c['type']=='Ready' and c['status']=='True' for c in worker.get('status',{}).get('conditions',[])), 'Ready native worker required: '+arch)
            require(self.nodes[worker['spec']['nodeName']]['status']['nodeInfo']['architecture']==arch, 'Worker is on the wrong real architecture')
        self.probes = {arch:base64.b64encode(verify_probe(Path(self.config['probes']).expanduser()/('probe-'+arch),arch)).decode() for arch in sorted(ARCHITECTURES)}
        self.report['nodes'] = [{k:n['status']['nodeInfo'][k] for k in ('architecture','operatingSystem','kubeletVersion','containerRuntimeVersion')}
            | {'name':n['metadata']['name'],'uid':n['metadata']['uid']} for n in nodes]

    def inventory(self, image):
        uid = image['metadata']['uid']
        children = self.get('images.kimage.dev',namespace=self.namespace,selector=PARENT_LABEL+'='+uid)['items']
        releases = self.get('imagereleases.kimage.dev',namespace=self.namespace)['items']
        demands = [d for d in self.get('imagedemands.kimage.dev',namespace=self.system,
            selector='kimage.dev/installation='+self.config['installation'])['items'] if d['spec']['imageUid']==uid]
        require(len(demands)<=1, 'Duplicate platform demand identity')
        return children, releases, demands

    def case(self, architecture):
        label = architecture or 'unknown'
        name = self.prefix+'-'+label
        expected = {architecture} if architecture else ARCHITECTURES
        self.create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name+'-source'),immutable=True,
            binaryData={'probe-'+a:p for a,p in self.probes.items()},data={'case.txt':name}))
        image = self.create(image_body(name,name+'-source'))
        require(image['metadata']['uid'] not in {c['uid'] for c in self.cases}, 'Fresh Image UID required')
        self.cases.append(dict(name=name,uid=image['metadata']['uid'],expected=expected))
        self.wait(lambda: any(c.get('reason')=='AwaitingDemand' for c in (self.get('images.kimage.dev',name,self.namespace).get('status') or {}).get('conditions',[])), name+' awaits demand')
        children, releases, demands = self.inventory(image)
        require(not children and not demands and not any(r['spec']['imageUid']==image['metadata']['uid'] for r in releases), 'Fresh Image already has a build, release, or demand')
        self.create(deployment_body(name,self.namespace,architecture))
        def published():
            value = self.get('images.kimage.dev',name,self.namespace)
            if is_ready(value):
                return value
            children = self.get('images.kimage.dev',namespace=self.namespace,selector=PARENT_LABEL+'='+image['metadata']['uid'])['items']
            for child in children:
                status = child.get('status') or {}
                if (status.get('operation') or {}).get('phase') == 'Failed':
                    raise RuntimeError('Native variant failed: '+str([c.get('reason') for c in status.get('conditions',[])]))
            return None
        image = self.wait(published,name+' native publication')
        children, releases, demands = self.inventory(image)
        require(len(demands)==1,'Missing platform demand')
        selected = assert_variants(image,children,releases,demands[0],expected)
        root = selected['spec']['root']['digest']
        def consumers():
            pods = self.get('pods',namespace=self.namespace,selector='kimage.dev/mixed-e2e-case='+name)['items']
            if len(pods)!=len(expected) or not all(any(c['type']=='Ready' and c['status']=='True' for c in p.get('status',{}).get('conditions',[])) for p in pods):
                return None
            observed = set()
            for pod in pods:
                arch = self.nodes[pod['spec']['nodeName']]['status']['nodeInfo']['architecture']
                require(pod['spec']['containers'][0]['image'].endswith('/'+self.namespace+'/'+image['metadata']['uid']+'@'+root), 'Consumer did not pull the selected native/index root')
                require(pod['spec']['containers'][0]['imagePullPolicy']=='IfNotPresent', 'Consumer pull policy changed')
                output = self.call(['logs',pod['metadata']['name'],'-n',self.namespace,'-c','app','--tail=5']).stdout.strip()
                require(output=='linux/'+arch,'Native workload did not execute its matching built binary')
                observed.add(arch)
            require(observed==expected,'Consumers did not run on every expected architecture')
            return pods
        pods = self.wait(consumers,name+' native kubelet pulls')
        current = self.get('deployments',name,self.namespace)
        require(current['spec']['template']['spec']['containers'][0]['image'].startswith('platform://'), 'Stored workload template was rewritten')
        self.report['cases'].append(dict(placement=label,image=name,imageUid=image['metadata']['uid'],
            platforms=sorted(expected),root=root,mediaType=selected['spec']['root']['mediaType'],
            childUids=sorted(c['metadata']['uid'] for c in children),consumerNodes=sorted(p['spec']['nodeName'] for p in pods)))
        self.save()
        print('PASS: fresh '+label+' placement built only '+', '.join(sorted(expected))+'; native consumers pulled '+('OCI index' if len(expected)==2 else 'manifest'),flush=True)

    def save(self):
        (self.output/'results.json').write_text(json.dumps(self.report,indent=2)+'\n')

    def run(self):
        try:
            self.preflight()
            for architecture in (None,'amd64','arm64'):
                self.case(architecture)
            time.sleep(self.config.get('settleSeconds',10))
            for case in self.cases:
                image = self.get('images.kimage.dev',case['name'],self.namespace)
                children,releases,demands = self.inventory(image)
                require(len(demands)==1,'Missing final demand')
                assert_variants(image,children,releases,demands[0],case['expected'])
            require(self.get('configmap',self.config['installation']+'-config',self.system)['data']['configuration.json']==self.configuration,'Administrator build policy changed')
            self.report['complete'] = True
        except Exception as error:
            self.report['failure'] = str(error)
            # Capture only this run's object identities and statuses. Do not
            # export Secrets, binary inputs, or unrelated workloads.
            diagnostics = []
            for case in self.cases:
                try:
                    image = self.get('images.kimage.dev',case['name'],self.namespace)
                    children, releases, demands = self.inventory(image)
                    objects = [image]+children+demands+[r for r in releases if r['spec']['imageUid'] in
                        {case['uid']} | {c['metadata']['uid'] for c in children}]
                    objects += self.get('pods',namespace=self.namespace,selector='kimage.dev/mixed-e2e-case='+case['name'])['items']
                    diagnostics.extend(dict(kind=o['kind'],name=o['metadata']['name'],uid=o['metadata']['uid'],
                        status=o.get('status',{})) for o in objects)
                except Exception as diagnostic_error:
                    diagnostics.append(dict(image=case['name'],error=str(diagnostic_error)))
            (self.output/'failure-statuses.json').write_text(json.dumps(diagnostics,indent=2)+'\n')
            raise
        finally:
            errors = []
            for name,uid in reversed(self.deployments):
                result = self.call(['delete','--raw','/apis/apps/v1/namespaces/'+self.namespace+'/deployments/'+name,'-f','-'],
                    dict(apiVersion='v1',kind='DeleteOptions',propagationPolicy='Foreground',preconditions=dict(uid=uid)),ok=False)
                if result.returncode and 'NotFound' not in result.stderr:
                    errors.append(name+': '+result.stderr[-500:])
            if not errors and self.deployments:
                try:
                    self.wait(lambda: not self.get('pods',namespace=self.namespace,selector=RUN_LABEL+'='+self.run_id)['items'], 'owned consumer cleanup')
                except Exception as error:
                    errors.append(str(error))
            self.report['retainedImages'] = [c['name'] for c in self.cases]
            self.report['cleanupComplete'] = not errors
            self.report['cleanupErrors'] = errors
            self.save()
            require(not errors,'Consumer cleanup failed: '+str(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run',action='store_true',help='Explicitly enable cluster mutations for this tagged E2E')
    parser.add_argument('--config',type=Path,help='Non-secret JSON settings; kubeconfig remains in its own file')
    parser.add_argument('--build-probes',type=Path,help='Build both static fixture executables locally without Kubernetes')
    args = parser.parse_args()
    if args.build_probes:
        build_probes(args.build_probes.resolve())
    if not args.run:
        print('SKIP: mixed-cluster E2E requires --run and --config; no Kubernetes access')
        return
    require(args.config is not None,'--config is required with --run')
    MixedCluster(json.loads(args.config.read_text())).run()


if __name__ == '__main__':
    main()
