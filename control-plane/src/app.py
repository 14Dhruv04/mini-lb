import hashlib
import json
import os
import secrets
import logging
from datetime import datetime, timezone
from decimal import Decimal

import boto3
from botocore.exceptions import ClientError
from opensearchpy import AWSV4SignerAuth, OpenSearch, RequestsHttpConnection

logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)

TABLE_NAME = os.environ["TABLE_NAME"]
ADMIN_TOKEN = os.environ.get("ADMIN_TOKEN", "")
STATE_MACHINE_ARN = os.environ["STATE_MACHINE_ARN"]
OPENSEARCH_ENDPOINT = os.environ.get("OPENSEARCH_ENDPOINT", "").rstrip("/")
OPENSEARCH_INDEX = os.environ.get("OPENSEARCH_INDEX", "minilb-events")
AWS_REGION = os.environ.get("AWS_REGION", "ap-south-1")

sfn = boto3.client("stepfunctions")

dynamodb = boto3.resource("dynamodb")
table = dynamodb.Table(TABLE_NAME)


def opensearch_client():
    if not OPENSEARCH_ENDPOINT:
        raise RuntimeError("OPENSEARCH_ENDPOINT is not configured")
    credentials = boto3.Session().get_credentials()
    if credentials is None:
        raise RuntimeError("unable to obtain AWS credentials for OpenSearch")
    auth = AWSV4SignerAuth(credentials, AWS_REGION, "aoss")
    endpoint = OPENSEARCH_ENDPOINT.replace("https://", "").replace("http://", "")
    return OpenSearch(
        hosts=[{"host": endpoint, "port": 443}],
        http_auth=auth,
        use_ssl=True,
        verify_certs=True,
        connection_class=RequestsHttpConnection,
        timeout=5,
        max_retries=2,
        retry_on_timeout=True,
    )


def now_iso():
    return datetime.now(timezone.utc).isoformat()


def _json_default(value):
    # boto3's DynamoDB resource layer returns Number values as Decimal.
    # Convert them into ordinary JSON numbers before returning an API response.
    if isinstance(value, Decimal):
        return int(value) if value == value.to_integral_value() else float(value)
    raise TypeError(f"Object of type {type(value).__name__} is not JSON serializable")


def response(status, body):
    return {
        "statusCode": status,
        "headers": {
            "content-type": "application/json",
            "access-control-allow-origin": "*",
            "access-control-allow-headers": "content-type,x-admin-token,x-minilb-token",
            "access-control-allow-methods": "GET,POST,PUT,OPTIONS",
        },
        "body": json.dumps(body, default=_json_default),
    }


def body_json(event):
    raw = event.get("body") or "{}"
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        raise ValueError("request body must be valid JSON")


def path_params(event):
    return event.get("pathParameters") or {}


def admin_ok(event):
    supplied = (event.get("headers") or {}).get("x-admin-token") or ""
    return bool(ADMIN_TOKEN) and secrets.compare_digest(supplied, ADMIN_TOKEN)


def token_hash(token):
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def mini_token_ok(event, stored_hash):
    supplied = (event.get("headers") or {}).get("x-minilb-token") or ""
    return bool(supplied) and secrets.compare_digest(token_hash(supplied), stored_hash)


from common import validate_config


def get_instance(instance_id):
    result = table.get_item(Key={"instanceId": instance_id})
    return result.get("Item")


def handle_create_instance(event):
    if not admin_ok(event):
        return response(401, {"message": "invalid admin token"})

    try:
        payload = body_json(event)
    except ValueError as exc:
        return response(400, {"message": str(exc)})

    instance_id = payload.get("instanceId") or f"lb_{secrets.token_hex(6)}"
    name = payload.get("name") or instance_id
    token = secrets.token_urlsafe(32)
    created = now_iso()
    try:
        config = validate_config(payload.get("config") or {"backends": []})
    except ValueError as exc:
        return response(400, {"message": str(exc)})
    item = {
        "instanceId": instance_id,
        "name": name,
        "tokenHash": token_hash(token),
        "version": 1,
        "createdAt": created,
        "updatedAt": created,
        **config,
    }

    try:
        table.put_item(Item=item, ConditionExpression="attribute_not_exists(instanceId)")
    except ClientError as exc:
        if exc.response.get("Error", {}).get("Code") == "ConditionalCheckFailedException":
            return response(409, {"message": "instance already exists"})
        raise

    return response(201, {
        "instanceId": instance_id,
        "name": name,
        "version": 1,
        "token": token,
        "config": config,
    })


def handle_list_instances(event):
    if not admin_ok(event):
        return response(401, {"message": "invalid admin token"})

    result = table.scan(ProjectionExpression="instanceId, #n, version, updatedAt", ExpressionAttributeNames={"#n": "name"})
    return response(200, {"instances": result.get("Items", [])})


def handle_get_instance(event, instance_id):
    if not admin_ok(event):
        return response(401, {"message": "invalid admin token"})
    item = get_instance(instance_id)
    if not item:
        return response(404, {"message": "instance not found"})
    safe = dict(item)
    safe.pop("tokenHash", None)
    return response(200, safe)


def handle_get_config(event, instance_id):
    item = get_instance(instance_id)
    if not item:
        return response(404, {"message": "instance not found"})
    if not mini_token_ok(event, item.get("tokenHash", "")):
        return response(401, {"message": "invalid MiniLB token"})

    return response(200, {
        "instance_id": instance_id,
        "version": int(item.get("version", 1)),
        "health_check_interval_seconds": int(item.get("health_check_interval_seconds", 5)),
        "health_check_path": item.get("health_check_path", "/health"),
        "backends": item.get("backends", []),
    })


def handle_put_config(event, instance_id):
    if not admin_ok(event):
        return response(401, {"message": "invalid admin token"})
    item = get_instance(instance_id)
    if not item:
        return response(404, {"message": "instance not found"})

    try:
        config = validate_config(body_json(event))
    except ValueError as exc:
        return response(400, {"message": str(exc)})

    current_version = int(item.get("version", 1))
    proposed_version = current_version + 1
    requested_at = now_iso()

    execution_input = {
        "instanceId": instance_id,
        "version": proposed_version,
        "config": config,
        "requestedAt": requested_at,
    }

    try:
        execution = sfn.start_execution(
            stateMachineArn=STATE_MACHINE_ARN,
            input=json.dumps(execution_input),
        )
    except ClientError as exc:
        logger.exception("failed to start configuration workflow")
        return response(500, {
            "message": "unable to start configuration workflow",
            "error": exc.response.get("Error", {}).get("Code", "Unknown"),
        })

    return response(202, {
        "instanceId": instance_id,
        "version": proposed_version,
        "status": "VALIDATING",
        "executionArn": execution.get("executionArn"),
        "config": config,
    })


def handle_post_event(event, instance_id):
    item = get_instance(instance_id)
    if not item:
        return response(404, {"message": "instance not found"})
    if not mini_token_ok(event, item.get("tokenHash", "")):
        return response(401, {"message": "invalid MiniLB token"})

    try:
        payload = body_json(event)
    except ValueError as exc:
        return response(400, {"message": str(exc)})

    document = {
        **payload,
        "instanceId": instance_id,
        "receivedAt": now_iso(),
    }

    # Use a deterministic OpenSearch document ID so retries are idempotent.
    # Request events use the MiniLB request_id when available; other event
    # types fall back to a stable hash of the event contents.
    request_id = str(payload.get("request_id") or "").strip()
    if request_id:
        document_id = f"{instance_id}:{request_id}"
    else:
        canonical = json.dumps(
            {k: v for k, v in document.items() if k != "receivedAt"},
            sort_keys=True,
            separators=(",", ":"),
        )
        document_id = f"{instance_id}:{hashlib.sha256(canonical.encode("utf-8")).hexdigest()}"

    document["eventId"] = document_id

    try:
        client = opensearch_client()
        result = client.index(
            index=OPENSEARCH_INDEX,
            id=document_id,
            body=document,
        )
        return response(202, {
            "status": "accepted",
            "eventId": result.get("_id", document_id),
            "index": result.get("_index", OPENSEARCH_INDEX),
        })
    except Exception:
        logger.exception("failed to index MiniLB telemetry")
        return response(503, {"message": "telemetry backend unavailable"})


def handle_get_events(event, instance_id):
    if not admin_ok(event):
        return response(401, {"message": "invalid admin token"})
    size_raw = ((event.get("queryStringParameters") or {}).get("size") or "20")
    try:
        size = max(1, min(int(size_raw), 100))
    except ValueError:
        return response(400, {"message": "size must be an integer between 1 and 100"})
    event_type = (event.get("queryStringParameters") or {}).get("type")

    filters = [{"term": {"instanceId.keyword": instance_id}}]
    if event_type:
        filters.append({"term": {"event_type.keyword": event_type}})

    query = {
        "size": size,
        "sort": [{"timestamp_ms": {"order": "desc", "unmapped_type": "long"}}],
        "query": {"bool": {"filter": filters}},
    }

    try:
        result = opensearch_client().search(index=OPENSEARCH_INDEX, body=query)
    except Exception:
        logger.exception("failed to search MiniLB telemetry")
        return response(503, {"message": "telemetry backend unavailable"})

    hits = result.get("hits", {}).get("hits", [])
    return response(200, {
        "events": [hit.get("_source", {}) for hit in hits],
        "total": result.get("hits", {}).get("total", {}).get("value", len(hits)),
    })


def lambda_handler(event, context):
    try:
        if (event.get("requestContext") or {}).get("http", {}).get("method") == "OPTIONS":
            return response(204, {})

        method = (event.get("requestContext") or {}).get("http", {}).get("method", "GET")
        path = (event.get("rawPath") or "/").rstrip("/") or "/"
        params = path_params(event)

        # Match the more specific /config route before the instance route.
        if method == "GET" and "instanceId" in params and path.endswith(f"/instances/{params['instanceId']}/config"):
            return handle_get_config(event, params["instanceId"])
        if method == "PUT" and "instanceId" in params and path.endswith(f"/instances/{params['instanceId']}/config"):
            return handle_put_config(event, params["instanceId"])
        if method == "POST" and "instanceId" in params and path.endswith(f"/instances/{params['instanceId']}/events"):
            return handle_post_event(event, params["instanceId"])
        if method == "GET" and "instanceId" in params and path.endswith(f"/instances/{params['instanceId']}/events"):
            return handle_get_events(event, params["instanceId"])
        if method == "POST" and path == "/instances":
            return handle_create_instance(event)
        if method == "GET" and path == "/instances":
            return handle_list_instances(event)
        if method == "GET" and "instanceId" in params and path.endswith(f"/instances/{params['instanceId']}"):
            return handle_get_instance(event, params["instanceId"])

        return response(404, {"message": "route not found"})
    except Exception:
        logger.exception("Unhandled MiniLB control-plane error")
        return response(500, {"message": "internal server error"})