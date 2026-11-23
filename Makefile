SHELL := /bin/bash
REGISTRY ?= 192.168.1.201:5000
TAG ?= $(shell awk '/^appVersion:/ {gsub(/[",]/, "", $$2); print $$2; exit}' deploy/helm/orbit/Chart.yaml)
NAMESPACE ?= orbit
KUBE_CONTEXT ?= microk8s
VALUES ?= deploy/helm/orbit/values.yaml
API_URL ?= http://localhost:8080

.PHONY: help dev-infra compose-up compose-down api worker web test test-db fmt clippy \
        typecheck build-web api-client helm-lint helm-template images push deploy smoke-k8s

help:
	@echo "ORBIT targets"
	@echo "  dev-infra      Start Postgres + NATS only (native development)"
	@echo "  compose-up     Full local stack via Docker Compose"
	@echo "  compose-logs   Follow api/worker/web logs"
	@echo "  compose-reset  Remove containers AND data volumes"
	@echo "  worker-opencode Run host OpenCode worker against the compose stack"
	@echo "  worker-docker  Spawn a Docker container per OpenCode execution (host dispatcher)"
	@echo "  worker-pi      Run a host Pi worker (Pi CLI + auth)"
	@echo "  compose-build-opencode Build the local OpenCode worker image"
	@echo "  api/worker     Run Rust services against the dev stack"
	@echo "  web            Run Next.js (API proxy to localhost:8080)"
	@echo "  test           Rust tests (DB tests skip without ORBIT_TEST_DATABASE_URL)"
	@echo "  test-db        Rust tests including PostgreSQL integration tests"
	@echo "  fmt/clippy     Rust formatting and lint gate"
	@echo "  typecheck      TypeScript typecheck for the web app"
	@echo "  images         Build linux/amd64 images for $(REGISTRY) tag $(TAG)"
	@echo "  push           Push images to $(REGISTRY)"
	@echo "  deploy         helm upgrade orbit ($(KUBE_CONTEXT), $(NAMESPACE), $(VALUES))"
	@echo "  helm-lint      helm lint + template validation"
	@echo "  smoke-k8s      End-to-end smoke test against $(API_URL)"

dev-infra:
	docker compose -f deploy/compose/compose.dev.yaml up -d

compose-up:
	docker compose -f deploy/compose/compose.yaml up --build

compose-down:
	docker compose -f deploy/compose/compose.yaml down

compose-logs:
	docker compose -f deploy/compose/compose.yaml logs -f --tail=100 api worker web

compose-reset:
	docker compose -f deploy/compose/compose.yaml down -v

worker-opencode:
	./scripts/dev-worker-opencode.sh

worker-docker:
	./scripts/dev-worker-docker.sh

worker-pi:
	./scripts/dev-worker-pi.sh

compose-build-opencode:
	docker build -f deploy/docker/worker.Dockerfile -t orbit-worker:dev .
	docker build --build-arg WORKER_BASE=orbit-worker:dev \
	  --build-arg OPENCODE_ARCH=$$( [ "$$(uname -m)" = "arm64" ] && echo arm64 || echo x64 ) \
	  -f deploy/docker/worker-opencode.Dockerfile -t orbit-worker-opencode:dev .

api:
	cargo run -p orbit-api-server

worker:
	cargo run -p orbit-worker

web:
	pnpm --filter @orbit/web dev

test:
	cargo test --workspace

test-db:
	ORBIT_TEST_DATABASE_URL=$${ORBIT_TEST_DATABASE_URL:-postgres://orbit:orbit@localhost:5432/orbit} cargo test --workspace

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

typecheck:
	pnpm --filter @orbit/web typecheck

build-web:
	pnpm --filter @orbit/web build

api-client:
	pnpm api:generate

helm-lint:
	helm lint deploy/helm/orbit -f deploy/helm/orbit/values.yaml \
	  --set auth.acknowledgeInsecureDev=true
	helm lint deploy/helm/orbit -f deploy/helm/orbit/values-prod.example.yaml \
	  --set auth.acknowledgeInsecureDev=true
	helm template orbit deploy/helm/orbit -n $(NAMESPACE) \
	  -f deploy/helm/orbit/values.yaml \
	  --set auth.acknowledgeInsecureDev=true > /tmp/orbit-rendered.yaml
	helm template orbit deploy/helm/orbit -n $(NAMESPACE) \
	  -f deploy/helm/orbit/values-prod.example.yaml \
	  --set auth.acknowledgeInsecureDev=true > /tmp/orbit-rendered-prod.yaml

helm-template:
	helm template orbit deploy/helm/orbit -n $(NAMESPACE) -f $(VALUES) \
	  --set auth.acknowledgeInsecureDev=true

images:
	REGISTRY=$(REGISTRY) TAG=$(TAG) ./scripts/build-images.sh

push:
	REGISTRY=$(REGISTRY) TAG=$(TAG) ./scripts/push-images.sh

deploy:
	REGISTRY=$(REGISTRY) TAG=$(TAG) NAMESPACE=$(NAMESPACE) KUBE_CONTEXT=$(KUBE_CONTEXT) VALUES=$(VALUES) ./scripts/deploy.sh

smoke-k8s:
	API_URL=$(API_URL) ./scripts/smoke-k8s.sh
