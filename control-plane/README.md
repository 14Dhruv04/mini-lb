# MiniLB control plane

Small AWS control plane for MiniLB using API Gateway HTTP API, Lambda, and DynamoDB.

## Deploy

Prerequisites:

- AWS CLI configured
- AWS SAM CLI installed

From `control-plane/`:

```bash
sam build
sam deploy --guided
```

Choose a strong `AdminToken` (at least 16 characters). This first version uses a shared admin token for management routes; it is intentionally an MVP and should not be treated as production authentication.

## API

Management routes use `X-Admin-Token`.

```text
POST /instances
GET  /instances
GET  /instances/{instanceId}
PUT  /instances/{instanceId}/config
```

MiniLB agents use their per-instance `X-MiniLB-Token` to read:

```text
GET /instances/{instanceId}/config
```

### Create instance

```json
{
  "instanceId": "dev-lb",
  "name": "local-dev",
  "config": {
    "health_check_interval_seconds": 5,
    "health_check_path": "/health",
    "backends": [
      {"id": "app-1", "url": "http://10.0.0.10:3000", "enabled": true},
      {"id": "app-2", "url": "http://10.0.0.11:3000", "enabled": true}
    ]
  }
}
```

The response includes the one-time MiniLB token. Keep it secret.
