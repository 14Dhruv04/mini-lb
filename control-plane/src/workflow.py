import json
import logging
import os
from datetime import datetime, timezone

import boto3
from botocore.exceptions import ClientError

from common import validate_config

logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)

TABLE_NAME = os.environ["TABLE_NAME"]
CONFIG_BUCKET = os.environ["CONFIG_BUCKET"]

dynamodb = boto3.resource("dynamodb")
table = dynamodb.Table(TABLE_NAME)
s3 = boto3.client("s3")


def now_iso():
    return datetime.now(timezone.utc).isoformat()


def validate(event, _context):
    """Validate the proposed configuration before it is persisted."""
    config = validate_config(event.get("config") or {})
    return {
        **event,
        "config": config,
        "validatedAt": now_iso(),
    }


def snapshot(event, _context):
    """Persist an immutable JSON snapshot before activation."""
    instance_id = event["instanceId"]
    version = int(event["version"])
    key = f"instances/{instance_id}/configs/v{version}.json"
    document = {
        "instanceId": instance_id,
        "version": version,
        "config": event["config"],
        "requestedAt": event.get("requestedAt"),
        "validatedAt": event.get("validatedAt"),
    }

    result = s3.put_object(
        Bucket=CONFIG_BUCKET,
        Key=key,
        Body=json.dumps(document, separators=(",", ":")).encode("utf-8"),
        ContentType="application/json",
        Metadata={"instance-id": instance_id, "config-version": str(version)},
    )

    return {
        **event,
        "snapshotKey": key,
        "snapshotVersionId": result.get("VersionId"),
        "snapshottedAt": now_iso(),
    }


def activate(event, _context):
    """Promote the snapshot to the active DynamoDB configuration atomically."""
    instance_id = event["instanceId"]
    version = int(event["version"])
    expected_version = version - 1
    config = event["config"]

    updated = now_iso()

    try:
        table.update_item(
            Key={"instanceId": instance_id},
            UpdateExpression=(
                "SET health_check_interval_seconds = :interval, "
                "health_check_path = :path, backends = :backends, "
                "version = :version, updatedAt = :updated, "
                "activeSnapshotKey = :snapshot_key"
            ),
            ExpressionAttributeValues={
                ":interval": config["health_check_interval_seconds"],
                ":path": config["health_check_path"],
                ":backends": config["backends"],
                ":version": version,
                ":updated": updated,
                ":snapshot_key": event["snapshotKey"],
                ":expected": expected_version,
            },
            ConditionExpression="version = :expected",
        )
    except ClientError as exc:
        if exc.response.get("Error", {}).get("Code") == "ConditionalCheckFailedException":
            raise RuntimeError(
                f"configuration version conflict for {instance_id}; "
                f"expected active version {expected_version}"
            ) from exc
        raise

    return {
        "instanceId": instance_id,
        "version": version,
        "snapshotKey": event["snapshotKey"],
        "snapshotVersionId": event.get("snapshotVersionId"),
        "activatedAt": updated,
    }
