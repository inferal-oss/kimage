{{- define "kimage.environment" -}}
{{- $root := . -}}
{{- $ns := .Release.Namespace -}}{{- $install := default .Release.Name .Values.installation -}}
{{- if or (eq $ns "default") (hasPrefix "kube-" $ns) (gt (len $install) 32) (not (regexMatch "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$" $install)) -}}{{- fail "Use a dedicated system namespace and an installation name of at most 32 DNS-label characters" -}}{{- end -}}
{{- $cluster := eq .Values.scope "cluster" -}}
{{- if not (has .Values.scope (list "namespace" "cluster")) -}}{{- fail "scope must be namespace or cluster" -}}{{- end -}}
{{- $app := default "default" .Values.applicationNamespace -}}
{{- if and $cluster .Values.applicationNamespace -}}{{- fail "applicationNamespace must be empty for cluster scope" -}}{{- end -}}
{{- if and (not $cluster) .Values.excludedNamespaces -}}{{- fail "excludedNamespaces requires cluster scope" -}}{{- end -}}
{{- $exclusions := concat (list $ns "kube-system" "kube-public" "kube-node-lease") .Values.excludedNamespaces | uniq | sortAlpha -}}
{{- if and (not $cluster) (or (has $app $exclusions) (not (regexMatch "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$" $app))) -}}{{- fail "applicationNamespace must name an ordinary namespace distinct from the system namespace" -}}{{- end -}}
{{- $scope := dict "systemNamespace" $ns "applicationNamespace" (ternary nil $app $cluster) "clusterWide" $cluster "excludedNamespaces" (.Values.excludedNamespaces | uniq | sortAlpha) -}}
{{- $selector := dict "matchLabels" (dict "kubernetes.io/metadata.name" $app) -}}
{{- $allowed := list $ns $app | uniq | sortAlpha -}}
{{- if $cluster -}}
  {{- $selector = dict "matchExpressions" (list (dict "key" "kubernetes.io/metadata.name" "operator" "NotIn" "values" $exclusions)) -}}
  {{- $allowed = list "*" "!kube-system" "!kube-public" "!kube-node-lease" -}}
  {{- range $excluded := .Values.excludedNamespaces -}}{{- if ne $excluded $ns -}}{{- $allowed = append $allowed (printf "!%s" $excluded) -}}{{- end -}}{{- end -}}
  {{- $allowed = $allowed | uniq | sortAlpha -}}
{{- end -}}
{{- $hooks := (lookup "admissionregistration.k8s.io/v1" "MutatingWebhookConfiguration" "" "" | default dict).items | default list -}}
{{- range $hook := $hooks -}}
  {{- if hasKey ($hook.metadata.labels | default dict) "kimage.dev/installation" -}}
    {{- range $webhook := ($hook.webhooks | default list) -}}
      {{- $service := $webhook.clientConfig.service | default dict -}}
      {{- if not $service.namespace -}}{{- fail "UnknownInstallationScope" -}}{{- end -}}
      {{- if not (and (eq $hook.metadata.name (printf "%s-resolution" $install)) (eq $service.namespace $ns)) -}}
        {{- $other := dict -}}
        {{- $annotation := get ($hook.metadata.annotations | default dict) "kimage.dev/scope" -}}
        {{- if $annotation -}}{{- $other = $annotation | fromJson -}}{{- else -}}
          {{- $appName := get (($webhook.namespaceSelector | default dict).matchLabels | default dict) "kubernetes.io/metadata.name" -}}
          {{- if not $appName -}}{{- fail "UnknownInstallationScope" -}}{{- end -}}
          {{- $other = dict "clusterWide" false "applicationNamespace" $appName "systemNamespace" $service.namespace "excludedNamespaces" list -}}
        {{- end -}}
        {{- $otherExcluded := concat (list $other.systemNamespace "kube-system" "kube-public" "kube-node-lease") ($other.excludedNamespaces | default list) -}}
        {{- $overlap := or (and $cluster $other.clusterWide) (and (not $cluster) (not (has $app $otherExcluded)) (or $other.clusterWide (eq $app $other.applicationNamespace))) (and (not $other.clusterWide) (not (has $other.applicationNamespace $exclusions)) (or $cluster (eq $app $other.applicationNamespace))) -}}
        {{- if $overlap -}}{{- fail "InstallationScopeOverlap: exclude the existing application's namespace or use a separate scope" -}}{{- end -}}
      {{- end -}}
    {{- end -}}
  {{- end -}}
{{- end -}}
{{- $nodes := .Values.nodes -}}
{{- if not $nodes -}}{{- $found := lookup "v1" "Node" "" "" | default dict -}}{{- $nodes = get $found "items" | default list -}}{{- end -}}
{{- $byName := dict -}}
{{- range $node := $nodes -}}
  {{- $status := $node.status | default dict -}}{{- $spec := $node.spec | default dict -}}
  {{- $ready := false -}}{{- range $condition := ($status.conditions | default list) -}}{{- if and (eq $condition.type "Ready") (eq $condition.status "True") -}}{{- $ready = true -}}{{- end -}}{{- end -}}
  {{- $info := $status.nodeInfo | default dict -}}
  {{- if and $ready (not $node.metadata.deletionTimestamp) (not $spec.unschedulable) (eq $info.operatingSystem "linux") (has $info.architecture (list "amd64" "arm64")) -}}
    {{- if or (ne (get $node.metadata.labels "kubernetes.io/os") $info.operatingSystem) (ne (get $node.metadata.labels "kubernetes.io/arch") $info.architecture) -}}{{- fail "NodePlatformLabelsMismatch" -}}{{- end -}}
    {{- $runtimeOK := or (and (eq $root.Values.runtimeProfile "inferal") (eq $info.kubeletVersion "v1.36.5+rke2r1") (eq $info.containerRuntimeVersion "containerd://2.3.4-k3s1")) (and (eq $root.Values.runtimeProfile "k3s") (eq $info.kubeletVersion "v1.36.5+k3s1") (eq $info.containerRuntimeVersion "containerd://2.3.4-k3s1.36")) -}}
    {{- if not $runtimeOK -}}{{- fail "UntestedRuntime: select the documented runtimeProfile" -}}{{- end -}}
    {{- if not $node.metadata.uid -}}{{- fail "MissingNodeUid" -}}{{- end -}}
    {{- $_ := set $byName $node.metadata.name $node -}}
  {{- end -}}
{{- end -}}
{{- $names := keys $byName | sortAlpha -}}
{{- if or (not $names) (gt (len $names) 128) -}}{{- fail "NoSupportedReadyNodes: Helm install must query the cluster; provide nodes only for offline rendering" -}}{{- end -}}
{{- if lookup "v1" "ConfigMap" $ns (printf "%s-state" $install) -}}{{- fail "LegacyInstallationDetected: automatic bootstrap migration is not supported" -}}{{- end -}}
{{- $prior := lookup "v1" "ConfigMap" $ns (printf "%s-installation" $install) -}}
{{- $priorRecord := dict -}}
{{- if $prior -}}
  {{- if or (ne (get $prior.metadata.labels "kimage.dev/installation") $install) (ne (get $prior.metadata.annotations "kimage.dev/manager") "helm") -}}{{- fail "InstallationManagerConflict" -}}{{- end -}}
  {{- $priorRecord = get $prior.data "installation.json" | fromJson -}}
{{- end -}}
{{- $priorNode := "" -}}{{- if $priorRecord -}}{{- $priorNode = $priorRecord.options.registryNode -}}{{- end -}}
{{- $primary := default (default (first $names) $priorNode) .Values.registryNode -}}
{{- if not (hasKey $byName $primary) -}}{{- fail "RegistryNodeNotEligible" -}}{{- end -}}
{{- if $priorRecord -}}
  {{- if ne $primary $priorNode -}}{{- fail "RegistryPlacementChangeRequiresStorageRecovery" -}}{{- end -}}
  {{- if ne $priorRecord.registryNodeUid (get $byName $primary).metadata.uid -}}{{- fail "RegistryNodeIdentityChanged: recover retained storage before replacing the node" -}}{{- end -}}
{{- end -}}
{{- $workers := dict -}}
{{- range $name := $names -}}{{- $node := get $byName $name -}}{{- $arch := $node.status.nodeInfo.architecture -}}{{- if not (hasKey $workers $arch) -}}{{- $_ := set $workers $arch $name -}}{{- end -}}{{- end -}}
{{- $retainedWorkers := dict -}}{{- if $priorRecord -}}{{- $retainedWorkers = $priorRecord.options.workerNodes | default dict -}}{{- end -}}
{{- range $arch, $name := mergeOverwrite $retainedWorkers .Values.workerNodes -}}
  {{- if not (hasKey $byName $name) -}}{{- fail "WorkerNodeNotEligible" -}}{{- end -}}
  {{- if ne (get $byName $name).status.nodeInfo.architecture $arch -}}{{- fail "WorkerNodePlatformMismatch" -}}{{- end -}}
  {{- $_ := set $workers $arch $name -}}
{{- end -}}
{{- $components := dict -}}{{- $images := dict -}}
{{- $platforms := list (dict "os" "linux" "architecture" "amd64") (dict "os" "linux" "architecture" "arm64") -}}
{{- range $role := list "admission" "builder" "exporter" "gateway" "input" "operator" "proxy" "registry" -}}
  {{- $digest := get $root.Values.images.digests $role | default "" -}}
  {{- if not (regexMatch "^sha256:[a-f0-9]{64}$" $digest) -}}{{- fail (printf "MissingComponentDigest: use a packaged release chart, or provide images.digests.%s" $role) -}}{{- end -}}
  {{- $version := $root.Chart.AppVersion -}}
  {{- if has $role (list "gateway" "proxy") -}}{{- $version = "0.3.0" -}}{{- else if eq $role "builder" -}}{{- $version = "0.33.0" -}}{{- else if eq $role "registry" -}}{{- $version = "3.1.2" -}}{{- end -}}
  {{- $_ := set $components $role (dict "version" $version "digest" $digest "platforms" $platforms) -}}
  {{- $_ := set $images $role (printf "%s/%s@%s" $root.Values.images.repository $role $digest) -}}
{{- end -}}
{{- $catalog := dict "schema" 1 "release" .Chart.AppVersion "apiSchemaRevision" 5 "repository" .Values.images.repository "components" $components -}}
{{- $registry := .Values.credentials.registry -}}
{{- if not $registry -}}{{- $old := lookup "v1" "Secret" $ns (printf "%s-registry-credentials" $install) -}}
  {{- if $old -}}{{- if or (ne (get $old.metadata.labels "kimage.dev/installation") $install) (ne (get $old.metadata.annotations "kimage.dev/manager") "helm") -}}{{- fail "ForeignCredentialSecret" -}}{{- end -}}{{- $registry = $old.data -}}{{- else -}}
    {{- $ca := genCA (printf "%s-registry.%s.svc CA" $install $ns) 3650 -}}
    {{- $leaf := genSignedCert (printf "%s-registry.%s.svc" $install $ns) nil (list (printf "%s-registry.%s.svc" $install $ns)) 3650 $ca -}}
    {{- $registry = dict "ca.crt" ($ca.Cert | b64enc) "tls.crt" ($leaf.Cert | b64enc) "tls.key" ($leaf.Key | b64enc) "reader-password" (randAlphaNum 64 | b64enc) "publisher-password" (randAlphaNum 64 | b64enc) -}}
  {{- end -}}
{{- end -}}
{{- $admission := .Values.credentials.admission -}}
{{- if not $admission -}}{{- $old := lookup "v1" "Secret" $ns (printf "%s-admission-credentials" $install) -}}
  {{- if $old -}}{{- if or (ne (get $old.metadata.labels "kimage.dev/installation") $install) (ne (get $old.metadata.annotations "kimage.dev/manager") "helm") -}}{{- fail "ForeignCredentialSecret" -}}{{- end -}}{{- $admission = $old.data -}}{{- else -}}
    {{- $ca := genCA (printf "%s-admission.%s.svc CA" $install $ns) 3650 -}}
    {{- $leaf := genSignedCert (printf "%s-admission.%s.svc" $install $ns) nil (list (printf "%s-admission.%s.svc" $install $ns)) 3650 $ca -}}
    {{- $admission = dict "ca.crt" ($ca.Cert | b64enc) "tls.crt" ($leaf.Cert | b64enc) "tls.key" ($leaf.Key | b64enc) "resolution-key" (randBytes 32) -}}
  {{- end -}}
{{- end -}}
{{- $builder := .Values.credentials.builder -}}
{{- if not $builder -}}{{- $old := lookup "v1" "Secret" $ns (printf "%s-builder-credentials" $install) -}}
  {{- if $old -}}{{- if or (ne (get $old.metadata.labels "kimage.dev/installation") $install) (ne (get $old.metadata.annotations "kimage.dev/manager") "helm") -}}{{- fail "ForeignCredentialSecret" -}}{{- end -}}{{- $builder = $old.data -}}{{- else -}}
    {{- $name := printf "%s-builder.%s.svc" $install $ns -}}
    {{- $ca := genCA (printf "%s CA" $name) 3650 -}}
    {{- $server := genSignedCert $name nil (list $name (printf "%s-builder-amd64.%s.svc" $install $ns) (printf "%s-builder-arm64.%s.svc" $install $ns)) 3650 $ca -}}
    {{- $client := genSignedCert (printf "%s-client" $name) nil nil 3650 $ca -}}
    {{- $builder = dict "ca.crt" ($ca.Cert | b64enc) "server.crt" ($server.Cert | b64enc) "server.key" ($server.Key | b64enc) "client.crt" ($client.Cert | b64enc) "client.key" ($client.Key | b64enc) -}}
  {{- end -}}
{{- end -}}
{{- range $entry := list (dict "material" $registry "keys" (list "ca.crt" "tls.crt" "tls.key" "reader-password" "publisher-password")) (dict "material" $admission "keys" (list "ca.crt" "tls.crt" "tls.key" "resolution-key")) (dict "material" $builder "keys" (list "ca.crt" "server.crt" "server.key" "client.crt" "client.key")) -}}
  {{- if ne ((keys $entry.material | sortAlpha) | toJson) (($entry.keys | sortAlpha) | toJson) -}}{{- fail "InvalidInstallationCredentialKeys: only the documented leaf material is accepted" -}}{{- end -}}
  {{- range $value := $entry.material -}}{{- if or (not $value) (gt (len ($value | b64dec)) 65536) -}}{{- fail "InvalidInstallationCredentialSize" -}}{{- end -}}{{- end -}}
{{- end -}}
{{- if or (eq (get $registry "reader-password") (get $registry "publisher-password")) (lt (len (get $admission "resolution-key" | b64dec)) 32) -}}{{- fail "IndependentInstallationCredentialsRequired" -}}{{- end -}}
{{- $storageClass := .Values.persistence.storageClass | default nil -}}{{- $existing := .Values.persistence.existingClaim | default nil -}}
{{- $options := dict "namespace" $ns "installation" $install "scope" .Values.scope "applicationNamespace" (ternary nil $app $cluster) "excludedNamespaces" (.Values.excludedNamespaces | uniq | sortAlpha) "runtimeProfile" .Values.runtimeProfile "registryNode" $primary "workerNodes" $workers "exportOrigins" (.Values.exporter.approvedOrigins | uniq | sortAlpha) "storageClass" $storageClass "storageSize" .Values.persistence.size "existingClaim" $existing "port" .Values.port -}}
{{- if $priorRecord -}}
  {{- if ne ((omit $options "workerNodes" "exportOrigins") | toJson) ((omit $priorRecord.options "workerNodes" "exportOrigins") | toJson) -}}{{- fail "InstallationConfigurationChanged: preserve scope, storage, port and registry placement" -}}{{- end -}}
{{- end -}}
{{- $buildPolicy := .Values.buildPolicy -}}
{{- $existingBuildRaw := "" -}}
{{- if not $buildPolicy -}}
  {{- $old := lookup "v1" "ConfigMap" $ns (printf "%s-config" $install) -}}
  {{- if $old -}}
    {{- $existingBuildRaw = get $old.data "configuration.json" -}}
    {{- $configuration := $existingBuildRaw | mustFromJson -}}
    {{- if or (ne (int $configuration.schema) 1) (gt (len (without (keys $configuration) "schema" "buildPolicy")) 0) -}}{{- fail "UnsupportedInstallationConfiguration" -}}{{- end -}}
    {{- $buildPolicy = $configuration.buildPolicy -}}
  {{- end -}}
{{- end -}}
{{- $buildPolicy = default "OnDemand" $buildPolicy -}}{{- if not (has $buildPolicy (list "OnDemand" "Eager")) -}}{{- fail "buildPolicy must be OnDemand or Eager" -}}{{- end -}}
{{- $buildConfiguration := dict "schema" 1 "buildPolicy" $buildPolicy | toJson -}}
{{- if $existingBuildRaw -}}{{- $buildConfiguration = $existingBuildRaw -}}{{- end -}}
{{- $origin := printf "https://%s-registry.%s.svc:5443/" $install $ns -}}
{{- $b := dict "manager" "helm" "namespace" $ns "installation" $install "applicationNamespace" (ternary nil $app $cluster) "primaryNode" $primary "port" .Values.port "registryOrigin" $origin "allowedNamespaces" (join "," $allowed) "scopeSelector" $selector "scopeJson" ($scope | toJson) "storageClass" $storageClass "storageSize" .Values.persistence.size "claimName" (default (printf "%s-data" $install) .Values.persistence.existingClaim) "registryConfiguration" "version: 0.1\nlog:\n  level: warn\nstorage:\n  filesystem:\n    rootdirectory: /data\n  delete:\n    enabled: false\nhttp:\n  addr: 127.0.0.1:5000\n  host: http://127.0.0.1:5000\n" "buildConfiguration" $buildConfiguration "imageCatalog" ($catalog | toJson) "installationConfiguration" (dict "schema" 1 "delivery" "ImagePull" "options" $options "registryNodeUid" (get $byName $primary).metadata.uid "release" .Chart.AppVersion | toJson) "registryCredentials" $registry "admissionCredentials" $admission "builderCredentials" $builder "proxyCredentials" (pick $registry "ca.crt" "reader-password") "publisherCredentials" (pick $registry "ca.crt" "publisher-password") "builderServerCredentials" (pick $builder "ca.crt" "server.crt" "server.key") "builderClientCredentials" (pick $builder "ca.crt" "client.crt" "client.key") "admissionCa" (get $admission "ca.crt") "workerCapabilities" (.Files.Get "files/resources/worker-capabilities.json" | fromJsonArray) -}}
{{- range $role, $image := $images -}}{{- $_ := set $b (printf "%sImage" $role) $image -}}{{- end -}}
{{- $op := list "--system-namespace" $ns "--node" $primary "--input-image" (get $images "input") "--registry-origin" $origin "--registry-ca" "/credentials/ca.crt" "--publisher-password" "/credentials/publisher-password" "--installation" $install "--builder-image" (get $images "builder") -}}
{{- $ad := list "--system-namespace" $ns "--installation" $install "--endpoint" (printf "127.0.0.1:%v" .Values.port) "--nodes" $primary "--cert" "/credentials/tls.crt" "--key" "/credentials/tls.key" "--resolution-key" "/credentials/resolution-key" "--operator-identity" (printf "system:serviceaccount:%s:%s-operator" $ns $install) "--input-image" (get $images "input") "--builder-image" (get $images "builder") -}}
{{- $scopeArgs := list "--namespace" $app -}}{{- if $cluster -}}{{- $scopeArgs = list "--cluster-wide" -}}{{- range $excluded := .Values.excludedNamespaces -}}{{- $scopeArgs = concat $scopeArgs (list "--exclude-namespace" $excluded) -}}{{- end -}}{{- end -}}
{{- $_ := set $b "operatorArguments" (concat $op $scopeArgs) -}}{{- $_ := set $b "admissionArguments" (concat $ad $scopeArgs) -}}
{{- $exports := .Values.exporter.approvedOrigins | uniq | sortAlpha -}}
{{- if gt (len $exports) 8 -}}{{- fail "TooManyApprovedExportOrigins" -}}{{- end -}}
{{- $exportArgs := list "--system-namespace" $ns "--installation" $install "--internal-origin" $origin "--internal-ca" "/credentials/ca.crt" "--internal-password" "/credentials/reader-password" -}}
{{- $exportArgs = concat $exportArgs $scopeArgs -}}
{{- range $origin := $exports -}}
  {{- if not (regexMatch "^https://[a-zA-Z0-9.-]+(:[0-9]+)?/$" $origin) -}}{{- fail "InvalidApprovedExportOrigin: use an HTTPS origin with trailing slash" -}}{{- end -}}
  {{- $exportArgs = concat $exportArgs (list "--approved-origin" $origin) -}}
{{- end -}}
{{- $_ := set $b "exporterArguments" $exportArgs -}}
{{- $_ := set $b "exporterRoleKind" (ternary "ClusterRole" "Role" $cluster) -}}
{{- $_ := set $b "exporterBindingKind" (ternary "ClusterRoleBinding" "RoleBinding" $cluster) -}}
{{- $_ := set $b "exporterApplicationNamespace" (ternary nil $app $cluster) -}}
{{- $_ := set $b "exporterApplicationName" (printf "%s-%s" $install (ternary "application-exporter" "exporter" $cluster)) -}}
{{- dict "bindings" $b "nodes" $byName "nodeNames" $names "workers" $workers "scope" $scope "options" $options "catalog" $catalog | toJson -}}
{{- end -}}
