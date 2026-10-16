{{/* Render the canonical JSON tree. Wrapping each value allows fromJson to
handle typed scalars and arrays on both Helm 3 and Helm 4. */}}
{{- define "kimage.render" -}}
{{- $v := .value -}}{{- $b := .bindings -}}
{{- if kindIs "map" $v -}}
  {{- $out := dict -}}
  {{- range $key, $child := $v -}}
    {{- $value := get (include "kimage.render" (dict "value" $child "bindings" $b) | fromJson) "value" -}}
    {{- if not (kindIs "invalid" $value) -}}{{- $_ := set $out $key $value -}}{{- end -}}
  {{- end -}}
  {{- dict "value" $out | toJson -}}
{{- else if kindIs "slice" $v -}}
  {{- $out := list -}}
  {{- range $child := $v -}}
    {{- $out = append $out (get (include "kimage.render" (dict "value" $child "bindings" $b) | fromJson) "value") -}}
  {{- end -}}
  {{- dict "value" $out | toJson -}}
{{- else if kindIs "string" $v -}}
  {{- if regexMatch "^\\$\\{[A-Za-z][A-Za-z0-9]*\\}$" $v -}}
    {{- $key := trimSuffix "}" (trimPrefix "${" $v) -}}
    {{- if not (hasKey $b $key) -}}{{- fail (printf "MissingInstallBinding: %s" $key) -}}{{- end -}}
    {{- dict "value" (get $b $key) | toJson -}}
  {{- else -}}
    {{- $out := $v -}}
    {{- range $key, $value := $b -}}
      {{- $token := printf "${%s}" $key -}}
      {{- if contains $token $out -}}
        {{- if or (kindIs "map" $value) (kindIs "slice" $value) (kindIs "invalid" $value) -}}{{- fail "NonScalarInstallInterpolation" -}}{{- end -}}
        {{- $out = replace $token (printf "%v" $value) $out -}}
      {{- end -}}
    {{- end -}}
    {{- if contains "${" $out -}}{{- fail "MissingInstallBinding" -}}{{- end -}}
    {{- dict "value" $out | toJson -}}
  {{- end -}}
{{- else -}}
  {{- dict "value" $v | toJson -}}
{{- end -}}
{{- end -}}

{{- define "kimage.resourceFile" -}}
{{- $raw := printf "{\"value\":%s}" (.root.Files.Get (printf "files/resources/%s" .name)) | fromJson -}}
{{- $resources := get (include "kimage.render" (dict "value" $raw.value "bindings" .bindings) | fromJson) "value" -}}
{{- range $resource := $resources }}
---
{{ $resource | toJson }}
{{ end -}}
{{- end -}}
