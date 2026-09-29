# MiniLB

MiniLB is a lightweight HTTP load balancer written in Rust. The data plane runs locally or in a container and supports round-robin routing, health checks, automatic failover, backend recovery, and runtime configuration reloads.

## Quick start with Docker

```bash
docker compose up --build
```

Traffic endpoint:

```bash
curl http://localhost:8080/api
```

Admin endpoint:

```bash
curl http://localhost:9090/status
```

See [DOCKER.md](DOCKER.md) for the plug-and-play demo and configuration.

## Current capabilities

- HTTP reverse proxy
- Round-robin load balancing
- Configurable health endpoint and interval
- Automatic removal of unhealthy backends
- Automatic reintegration of recovered backends
- YAML configuration
- Runtime configuration reload by file change or admin API
- Backend/status inspection through the admin API

## Design direction

MiniLB separates the fast request path from a future cloud control plane:

- Rust MiniLB: request routing and health state
- DynamoDB: backend/configuration state
- Step Functions: configuration validation/deployment workflows
- S3: configuration snapshots and durable artifacts
- OpenSearch: request and incident analytics
- React: management dashboard

## Cloud control plane (v0.3)

The `control-plane/` directory contains an AWS SAM application using API Gateway HTTP API, Lambda, and DynamoDB. MiniLB can optionally poll a remote config endpoint using `control_plane` in its YAML config. The local file remains the bootstrap source; remote versions are applied only when newer.
