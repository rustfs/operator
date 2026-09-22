{{/*
Copyright 2026 RustFS Team

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/}}
{{- define "rustfs-tenant.name" -}}
{{- $name := default .Release.Name .Values.tenant.metadata.name -}}
{{- if or (gt (len $name) 55) (not (regexMatch "^[a-z]([-a-z0-9]*[a-z0-9])?$" $name)) -}}
{{- fail "tenant.metadata.name (or release name) must be a DNS-1035 label of at most 55 characters" -}}
{{- end -}}
{{- $name -}}
{{- end -}}

{{/* Validate offline too: helm template must not depend on cluster discovery. */}}
{{- define "rustfs-tenant.validate" -}}
{{- range $endpoint := list "api" "console" -}}
{{- $ingress := index $.Values.ingress $endpoint -}}
{{- $route := index $.Values.httpRoute $endpoint -}}
{{- if and $ingress.enabled $route.enabled -}}
{{- fail (printf "%s: enable either ingress or httpRoute, not both" $endpoint) -}}
{{- end -}}
{{- if and $ingress.enabled (empty $ingress.hosts) -}}
{{- fail (printf "ingress.%s.hosts must not be empty when enabled" $endpoint) -}}
{{- end -}}
{{- if and $route.enabled (or (empty $route.parentRefs) (empty $route.hostnames)) -}}
{{- fail (printf "httpRoute.%s requires parentRefs and hostnames when enabled" $endpoint) -}}
{{- end -}}
{{- end -}}
{{- end -}}
