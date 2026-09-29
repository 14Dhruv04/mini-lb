# MiniLB with Docker

## Run the complete demo

```bash
docker compose up --build
```

Traffic enters the load balancer on `http://localhost:8080`.
The lightweight admin API listens on `http://localhost:9090`.

Test traffic:

```bash
curl http://localhost:8080/api
```

Repeat the request several times. Responses should alternate between `backend-a` and `backend-b`.

## Inspect MiniLB status

```bash
curl http://localhost:9090/status
```

The response includes the active backend list and health state.

## Reload configuration

MiniLB watches the configured YAML file and reloads it when the file changes.
You can also force a reload:

```bash
curl -X POST http://localhost:9090/reload
```

For example, add a third backend to `minilb.docker.yml`:

```yaml
  - id: "backend-c"
    url: "http://backend-c:8003"
    enabled: true
```

Once that backend exists on the Docker network, MiniLB picks it up without restarting the load balancer.

## Test failover

Stop one backend:

```bash
docker compose stop backend-a
```

Within the health-check interval, MiniLB removes it from the pool and requests continue through `backend-b`.

Restart it:

```bash
docker compose start backend-a
```

It will be re-added after a successful health check.

## Custom configuration

Mount your own config file at `/etc/minilb/minilb.yml` or set `MINILB_CONFIG` to another path.

Example:

```yaml
listen: "0.0.0.0:8080"
admin_listen: "0.0.0.0:9090"
health_check_interval_seconds: 5
health_check_path: "/health"
backends:
  - id: "my-api-1"
    url: "http://my-api-1:3000"
  - id: "my-api-2"
    url: "http://my-api-2:3000"
```

The current strategy is round-robin. Authentication for the admin API is intentionally not included in this MVP; do not expose port `9090` to the public internet.
