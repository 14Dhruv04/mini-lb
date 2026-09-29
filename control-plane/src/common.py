def validate_config(payload):
    backends = payload.get("backends")
    if not isinstance(backends, list) or not backends:
        raise ValueError("backends must be a non-empty array")

    seen = set()
    normalized = []
    for backend in backends:
        if not isinstance(backend, dict):
            raise ValueError("each backend must be an object")
        backend_id = backend.get("id")
        url = backend.get("url")
        if not isinstance(backend_id, str) or not backend_id:
            raise ValueError("backend id is required")
        if backend_id in seen:
            raise ValueError(f"duplicate backend id: {backend_id}")
        if not isinstance(url, str) or not (url.startswith("http://") or url.startswith("https://")):
            raise ValueError(f"backend {backend_id} must use an http(s) URL")
        seen.add(backend_id)
        normalized.append({
            "id": backend_id,
            "url": url,
            "enabled": bool(backend.get("enabled", True)),
        })

    interval = int(payload.get("health_check_interval_seconds", 5))
    if interval < 1 or interval > 3600:
        raise ValueError("health_check_interval_seconds must be between 1 and 3600")

    path = payload.get("health_check_path", "/health")
    if not isinstance(path, str) or not path.startswith("/"):
        raise ValueError("health_check_path must start with '/'")

    return {
        "health_check_interval_seconds": interval,
        "health_check_path": path,
        "backends": normalized,
    }
