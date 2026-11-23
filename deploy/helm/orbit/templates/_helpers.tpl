{{/*
ORBIT Helm helpers.
*/}}

{{- define "orbit.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "orbit.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "orbit.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "orbit.labels" -}}
helm.sh/chart: {{ include "orbit.chart" . }}
app.kubernetes.io/name: {{ include "orbit.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: orbit
{{- end -}}

{{- define "orbit.selectorLabels" -}}
app.kubernetes.io/name: {{ include "orbit.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "orbit.componentLabels" -}}
{{ include "orbit.selectorLabels" . }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "orbit.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (printf "%s" (include "orbit.fullname" .)) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{- define "orbit.secretName" -}}
{{- default (printf "%s-secrets" (include "orbit.fullname" .)) .Values.secrets.existingSecret -}}
{{- end -}}

{{- define "orbit.databaseUrl" -}}
{{- if .Values.secrets.databaseUrl -}}
{{- .Values.secrets.databaseUrl -}}
{{- else if and .Values.externalDatabase.enabled (not .Values.externalDatabase.existingSecret) -}}
{{- required "externalDatabase.url is required when externalDatabase.enabled=true and externalDatabase.existingSecret is empty" .Values.externalDatabase.url -}}
{{- else -}}
{{- printf "postgres://%s:%s@%s-postgresql:5432/%s?sslmode=disable" .Values.postgresql.user .Values.postgresql.password (include "orbit.fullname" .) .Values.postgresql.database -}}
{{- end -}}
{{- end -}}

{{- define "orbit.natsUrl" -}}
{{- if .Values.externalNats.enabled -}}
{{- required "externalNats.url is required when externalNats.enabled=true" .Values.externalNats.url -}}
{{- else -}}
{{- printf "nats://%s-nats:4222" (include "orbit.fullname" .) -}}
{{- end -}}
{{- end -}}

{{/*
Image reference. The tag defaults to the chart appVersion so a released chart always
pulls the images built for that release. Override per component with `<component>.tag`.
Call with: dict "root" $ "image" .Values.api.image "registry" .Values.imageRegistry "tag" .Values.api.tag
*/}}
{{- define "orbit.image" -}}
{{- $registry := trimSuffix "/" .registry -}}
{{- $tag := .tag -}}
{{- if not $tag -}}
{{- $tag = .root.Chart.AppVersion -}}
{{- end -}}
{{- if $registry -}}
{{- printf "%s/%s:%s" $registry .image $tag -}}
{{- else -}}
{{- printf "%s:%s" .image $tag -}}
{{- end -}}
{{- end -}}

{{- define "orbit.apiHost" -}}
{{- printf "%s-api" (include "orbit.fullname" .) -}}
{{- end -}}

{{- define "orbit.webHost" -}}
{{- printf "%s-web" (include "orbit.fullname" .) -}}
{{- end -}}

{{/*
Tag used for generated worker-profile image defaults. Mirrors the API image tag so a
profile seeded at startup references the same release the chart deployed.
*/}}
{{- define "orbit.imageTag" -}}
{{- if .Values.worker.tag -}}
{{- .Values.worker.tag -}}
{{- else if .Values.api.tag -}}
{{- .Values.api.tag -}}
{{- else -}}
{{- .Chart.AppVersion -}}
{{- end -}}
{{- end -}}

{{/*
Registry credentials for pods created by the launcher. Falls back to the first
imagePullSecrets entry so a private registry works without configuring it twice.
*/}}
{{- define "orbit.launcherImagePullSecret" -}}
{{- if .Values.launcher.imagePullSecret -}}
{{- .Values.launcher.imagePullSecret -}}
{{- else if .Values.imagePullSecrets -}}
{{- (index .Values.imagePullSecrets 0).name -}}
{{- end -}}
{{- end -}}

{{/*
Launcher bearer token shared by orbit-api and orbit-launcher.

Precedence: explicit launcher.token > value already stored in the release Secret >
newly generated. The lookup keeps the token stable across upgrades (and lets the
launcher restart keep authenticating) without requiring the operator to handle it.

GitOps caveat: `helm template`/rendering has no cluster access, so the generated value
differs per render. Set `launcher.token` explicitly (or put `launcher-token` in
`secrets.existingSecret`) when the rendered output is committed or diffed.
*/}}
{{- define "orbit.launcherToken" -}}
{{- if .Values.launcher.token -}}
{{- .Values.launcher.token -}}
{{- else -}}
{{- $secret := lookup "v1" "Secret" .Release.Namespace (include "orbit.secretName" .) -}}
{{- if and $secret $secret.data (index $secret.data "launcher-token") -}}
{{- index $secret.data "launcher-token" | b64dec -}}
{{- else -}}
{{- randAlphaNum 48 -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
Production guardrails. Included from templates/validate.yaml so it runs on every render.
*/}}
{{- define "orbit.validate" -}}
{{- if and .Values.monitoring.annotations .Values.monitoring.token.existingSecret -}}
{{- fail "monitoring.annotations=true cannot be combined with monitoring.token.existingSecret: annotation-based discovery cannot send a bearer token. Use monitoring.serviceMonitor.enabled=true, or drop the token and restrict the metrics port with monitoring.port and a NetworkPolicy." -}}
{{- end -}}
{{- if and .Values.monitoring.serviceMonitor.enabled .Values.monitoring.token.existingSecret .Values.monitoring.serviceMonitor.namespace (ne (toString .Values.monitoring.serviceMonitor.namespace) .Release.Namespace) -}}
{{- fail "monitoring.serviceMonitor.namespace must be the release namespace when monitoring.token.existingSecret is set: the ServiceMonitor reads the token Secret from its own namespace." -}}
{{- end -}}
{{- if and .Values.ingress.enabled (eq (toString .Values.auth.mode) "dev") .Values.auth.dev.allowAnonymous (not .Values.auth.acknowledgeInsecureDev) -}}
{{- fail "ingress.enabled=true with auth.mode=dev and auth.dev.allowAnonymous=true exposes an anonymous Owner UI. Use auth.mode=oidc (see values-prod.example.yaml), set auth.dev.allowAnonymous=false, disable the ingress, or set auth.acknowledgeInsecureDev=true for a throwaway local/test cluster." -}}
{{- end -}}
{{- if eq (toString .Values.platform.environment) "production" -}}
{{- if and .Values.secrets.create (not .Values.secrets.existingSecret) (eq (toString .Values.secrets.masterKey) "b3JiaXQtZGV2LW1hc3Rlci1rZXktMzItYnl0ZXMhISE=") -}}
{{- fail "platform.environment=production cannot use the published development secrets.masterKey. Generate one with `openssl rand -base64 32` or set secrets.existingSecret." -}}
{{- end -}}
{{- if eq (toString .Values.auth.mode) "dev" -}}
{{- fail "platform.environment=production does not allow auth.mode=dev; configure auth.mode=oidc." -}}
{{- end -}}
{{- if and .Values.postgresql.enabled (eq (toString .Values.postgresql.password) "orbit") -}}
{{- fail "platform.environment=production with the bundled PostgreSQL requires a non-default postgresql.password (or use externalDatabase.enabled=true)." -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
Port name and number Prometheus scrapes: the dedicated metrics port when
`monitoring.port` is set, otherwise the API port.
*/}}
{{- define "orbit.metricsPortName" -}}
{{- if .Values.monitoring.port -}}metrics{{- else -}}http{{- end -}}
{{- end -}}
{{- define "orbit.metricsPort" -}}
{{- if .Values.monitoring.port -}}{{ .Values.monitoring.port }}{{- else -}}{{ .Values.api.port }}{{- end -}}
{{- end -}}

{{/* prometheus.io/* discovery annotations for the API pod and Service. */}}
{{- define "orbit.scrapeAnnotations" -}}
prometheus.io/scrape: "true"
prometheus.io/path: "/metrics"
prometheus.io/port: {{ include "orbit.metricsPort" . | quote }}
{{- end -}}
