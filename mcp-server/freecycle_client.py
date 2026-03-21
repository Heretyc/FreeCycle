"""FreeCycle GPU Lifecycle Manager — Python Client Library.

Together with the Node.js MCP server, this is one of the two ONLY supported
interfaces for interacting with FreeCycle and its local inference engine.  Do not
access the Ollama API or FreeCycle agent server directly — this client implements
TOFU TLS pinning, task signaling, wake-on-LAN, and multi-server routing that
cannot be replicated with raw HTTP calls.

Every MCP tool has a 1:1 method on FreeCycleClient (async + sync):

    MCP Tool                   -> Async Method            -> Sync Method
    freecycle_status           -> client.status()         -> client.status_sync()
    freecycle_health           -> client.health()         -> client.health_sync()
    freecycle_check_availability -> client.check_availability() -> client.check_availability_sync()
    freecycle_start_task       -> client.start_task()     -> client.start_task_sync()
    freecycle_stop_task        -> client.stop_task()      -> client.stop_task_sync()
    freecycle_list_models      -> client.list_models()    -> client.list_models_sync()
    freecycle_show_model       -> client.show_model()     -> client.show_model_sync()
    freecycle_pull_model       -> client.pull_model()     -> client.pull_model_sync()
    freecycle_generate         -> client.generate()       -> client.generate_sync()
    freecycle_chat             -> client.chat()           -> client.chat_sync()
    freecycle_embed            -> client.embed()          -> client.embed_sync()
    freecycle_evaluate_task    -> client.evaluate_task()  -> client.evaluate_task_sync()
    freecycle_benchmark        -> client.benchmark()      -> client.benchmark_sync()
    freecycle_add_server       -> client.add_server()     -> client.add_server_sync()
    freecycle_list_servers     -> client.list_servers()    -> client.list_servers_sync()
    freecycle_model_catalog    -> client.model_catalog()  -> client.model_catalog_sync()

All methods produce identical JSON payloads to their MCP counterparts. Use this
client over MCP tool calls in Python scripts, benchmark harnesses, and persistent
routing code to reduce token usage and latency.

Example (async):
    >>> import asyncio
    >>> from freecycle_client import FreeCycleClient
    >>> async def main():
    ...     client = FreeCycleClient()
    ...     status = await client.status()
    ...     print(status)
    >>> asyncio.run(main())

Example (sync):
    >>> from freecycle_client import FreeCycleClient
    >>> client = FreeCycleClient()
    >>> status = client.status_sync()
    >>> print(status)

Example (CLI):
    >>> python freecycle_client.py status
    >>> python freecycle_client.py generate --model llama3.1:8b-instruct-q4_K_M --prompt "Hello"
    >>> python freecycle_client.py --pretty list-models
"""

import asyncio
import argparse
import concurrent.futures
import hashlib
import http.client
import json
import logging
import os
import socket
import ssl
import sys
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Awaitable, Callable, Optional, Union
from urllib.parse import urlparse

__version__ = "2.0.1"

logger = logging.getLogger("freecycle_client")

# ============================================================================
# Constants
# ============================================================================

# Default configuration values (mirror Node.js config.ts)
DEFAULT_FC_HOST = "localhost"
DEFAULT_FC_PORT = 7443
DEFAULT_ENGINE_HOST = "localhost"
DEFAULT_ENGINE_PORT = 11434
DEFAULT_REQUEST_SECS = 10
DEFAULT_INFERENCE_SECS = 300
DEFAULT_PULL_SECS = 600
DEFAULT_WOL_ENABLED = False
DEFAULT_WOL_MAC = ""
DEFAULT_WOL_BROADCAST = "255.255.255.255"
DEFAULT_WOL_PORT = 9
DEFAULT_WOL_PACKET_COUNT = 5
DEFAULT_WOL_PACKET_INTERVAL_SECS = 0.25
DEFAULT_WOL_POLL_INTERVAL_SECS = 30
DEFAULT_WOL_MAX_WAIT_SECS = 900

# Cache TTLs
PROTOCOL_CACHE_TTL_SECS = 300  # 5 minutes — protocol detection (https/http)
MODEL_CACHE_TTL_SECS = 300      # 5 minutes — per-server model lists

# Immediate fallback statuses (mirrors availability.ts)
IMMEDIATE_FALLBACK_STATUSES: frozenset[str] = frozenset({
    "Blocked (Game Running)",
    "Cooldown",
    "Wake Delay",
    "Error",
})

# Keyword lists for evaluate_task (exact parity with spec)
LOCAL_KEYWORDS = [
    "summarize", "summarization", "summary",
    "embed", "embedding", "embeddings",
    "classify", "classification",
    "explain", "explanation",
    "translate", "translation",
    "extract", "extraction",
    "sentiment",
    "tag", "label",
    "rewrite", "paraphrase",
    "simple", "basic",
]

CLOUD_KEYWORDS = [
    "complex code", "advanced code", "code generation",
    "math proof", "theorem", "formal verification",
    "research", "analysis", "deep reasoning",
    "creative writing", "novel", "story",
    "multi-step reasoning",
    "planning", "architecture", "system design",
]

# ============================================================================
# Exception Classes
# ============================================================================

class FreeCycleError(Exception):
    """Base exception for FreeCycle client errors."""
    pass

class FreeCycleConnectionError(FreeCycleError):
    """Connection or protocol error."""
    pass

class FreeCycleTimeoutError(FreeCycleError):
    """Request timeout."""
    pass

class FreeCycleUnavailableError(FreeCycleError):
    """Local inference engine or FreeCycle server is unavailable."""
    pass

class TLSFingerprintMismatchError(FreeCycleError):
    """TLS certificate fingerprint mismatch."""
    pass

class TaskConflictError(FreeCycleError):
    """Task conflict (409 from server)."""
    pass

class ConfigError(FreeCycleError):
    """Configuration error."""
    pass

# ============================================================================
# Dataclasses
# ============================================================================

@dataclass
class EndpointConfig:
    """Configuration for a network endpoint."""
    host: str = DEFAULT_FC_HOST
    port: int = DEFAULT_FC_PORT

    def __post_init__(self):
        if not isinstance(self.port, int):
            self.port = int(self.port)

@dataclass
class TimeoutConfig:
    """Timeout settings (in seconds)."""
    request_secs: int = DEFAULT_REQUEST_SECS
    inference_secs: int = DEFAULT_INFERENCE_SECS
    pull_secs: int = DEFAULT_PULL_SECS

    def __post_init__(self):
        if not isinstance(self.request_secs, int):
            self.request_secs = int(self.request_secs)
        if not isinstance(self.inference_secs, int):
            self.inference_secs = int(self.inference_secs)
        if not isinstance(self.pull_secs, int):
            self.pull_secs = int(self.pull_secs)

@dataclass
class WakeOnLanConfig:
    """Wake-on-LAN settings."""
    enabled: bool = DEFAULT_WOL_ENABLED
    mac_address: str = DEFAULT_WOL_MAC
    broadcast_address: str = DEFAULT_WOL_BROADCAST
    port: int = DEFAULT_WOL_PORT
    packet_count: int = DEFAULT_WOL_PACKET_COUNT
    packet_interval_secs: float = DEFAULT_WOL_PACKET_INTERVAL_SECS
    poll_interval_secs: int = DEFAULT_WOL_POLL_INTERVAL_SECS
    max_wait_secs: int = DEFAULT_WOL_MAX_WAIT_SECS

@dataclass
class ServerEntry:
    """Configuration for a FreeCycle server."""
    host: str
    port: int
    name: Optional[str] = None
    approved: bool = False
    tls_fingerprint: Optional[str] = None
    identity_uuid: Optional[str] = None
    wake_on_lan: Optional[WakeOnLanConfig] = None
    timeouts: Optional[TimeoutConfig] = None

@dataclass
class McpServerConfig:
    """Complete MCP server configuration."""
    servers: list[ServerEntry] = field(default_factory=list)
    engine: EndpointConfig = field(default_factory=EndpointConfig)
    timeouts: TimeoutConfig = field(default_factory=TimeoutConfig)
    wake_on_lan: WakeOnLanConfig = field(default_factory=WakeOnLanConfig)

@dataclass
class LocalAvailability:
    """Result of availability check."""
    available: bool
    wake_on_lan_enabled: bool = False
    wake_on_lan_attempted: bool = False
    freecycle_reachable: bool = False
    engine_reachable: bool = False
    freecycle_status: Optional[str] = None
    blocking_processes: list[str] = field(default_factory=list)
    reason: str = ""

@dataclass
class ServerProbeResult:
    """Successful server probe."""
    server: ServerEntry
    status: dict
    reachable: bool = True
    free_vram_mb: float = 0.0

@dataclass
class ServerProbeError:
    """Failed server probe."""
    server: ServerEntry
    reachable: bool = False
    error: str = ""

@dataclass
class TrackedOperationOptions:
    """Options for tracked operations."""
    action: str
    operation_label: str
    model_name: Optional[str] = None
    availability: Optional[LocalAvailability] = None
    detail: Optional[str] = None
    server: Optional[ServerEntry] = None

# ============================================================================
# Config Loading and Writing
# ============================================================================

CONFIG_PATH = Path(__file__).parent / "freecycle-mcp.config.json"
_config_cache: Optional[McpServerConfig] = None

def _parse_number(val: Optional[str], default: Union[int, float]) -> Union[int, float]:
    """Safely parse environment variable to number."""
    if val is None:
        return default
    try:
        if isinstance(default, int):
            return int(val)
        return float(val)
    except ValueError:
        return default

def _parse_boolean(val: Optional[str], default: bool = False) -> bool:
    """Safely parse environment variable to boolean."""
    if val is None:
        return default
    return val.lower() in ("true", "1", "yes", "on")

def _read_raw_config() -> dict:
    """Load raw JSON config from file, return empty dict if missing."""
    path = get_config_path()
    if not path.exists():
        return {}
    try:
        with open(path, "r") as f:
            return json.load(f)
    except Exception as e:
        logger.warning(f"Failed to read config: {e}")
        return {}

def _normalize_servers_array(raw: dict) -> list[dict]:
    """Convert freecycle key or servers array to servers list."""
    if "servers" in raw and isinstance(raw["servers"], list):
        return raw["servers"]
    if "freecycle" in raw and isinstance(raw["freecycle"], dict):
        return [raw["freecycle"]]
    return []

def _parse_server_entry(data: dict) -> ServerEntry:
    """Convert raw dict to ServerEntry."""
    wol_dict = data.get("wakeOnLan") or data.get("wake_on_lan") or {}
    wol = WakeOnLanConfig(
        enabled=wol_dict.get("enabled", DEFAULT_WOL_ENABLED),
        mac_address=wol_dict.get("macAddress") or wol_dict.get("mac_address") or DEFAULT_WOL_MAC,
        broadcast_address=wol_dict.get("broadcastAddress") or wol_dict.get("broadcast_address") or DEFAULT_WOL_BROADCAST,
        port=int(wol_dict.get("port", DEFAULT_WOL_PORT)),
        packet_count=int(wol_dict.get("packetCount") or wol_dict.get("packet_count") or DEFAULT_WOL_PACKET_COUNT),
        packet_interval_secs=float(wol_dict.get("packetIntervalSecs") or wol_dict.get("packet_interval_secs") or DEFAULT_WOL_PACKET_INTERVAL_SECS),
        poll_interval_secs=int(wol_dict.get("pollIntervalSecs") or wol_dict.get("poll_interval_secs") or DEFAULT_WOL_POLL_INTERVAL_SECS),
        max_wait_secs=int(wol_dict.get("maxWaitSecs") or wol_dict.get("max_wait_secs") or DEFAULT_WOL_MAX_WAIT_SECS),
    )

    timeouts_dict = data.get("timeouts") or {}
    timeouts = TimeoutConfig(
        request_secs=int(timeouts_dict.get("requestSecs") or timeouts_dict.get("request_secs") or DEFAULT_REQUEST_SECS),
        inference_secs=int(timeouts_dict.get("inferenceSecs") or timeouts_dict.get("inference_secs") or DEFAULT_INFERENCE_SECS),
        pull_secs=int(timeouts_dict.get("pullSecs") or timeouts_dict.get("pull_secs") or DEFAULT_PULL_SECS),
    )

    return ServerEntry(
        host=data.get("host", DEFAULT_FC_HOST),
        port=int(data.get("port", DEFAULT_FC_PORT)),
        name=data.get("name"),
        approved=data.get("approved", False),
        tls_fingerprint=data.get("tlsFingerprint") or data.get("tls_fingerprint"),
        identity_uuid=data.get("identityUuid") or data.get("identity_uuid"),
        wake_on_lan=wol,
        timeouts=timeouts,
    )

def _merge_config(raw: dict) -> McpServerConfig:
    """Merge raw dict with env var overrides and defaults."""
    servers = _normalize_servers_array(raw)
    parsed_servers = [_parse_server_entry(s) for s in servers]

    engine_dict = raw.get("engine", {})
    engine_host = os.environ.get("ENGINE_HOST") or engine_dict.get("host") or DEFAULT_ENGINE_HOST
    engine_port = int(os.environ.get("ENGINE_PORT") or engine_dict.get("port") or DEFAULT_ENGINE_PORT)
    engine = EndpointConfig(host=engine_host, port=engine_port)

    timeouts_dict = raw.get("timeouts", {})
    timeouts = TimeoutConfig(
        request_secs=int(_parse_number(os.environ.get("FREECYCLE_REQUEST_TIMEOUT_SECS"), timeouts_dict.get("requestSecs") or DEFAULT_REQUEST_SECS)),
        inference_secs=int(_parse_number(os.environ.get("FREECYCLE_INFERENCE_TIMEOUT_SECS"), timeouts_dict.get("inferenceSecs") or DEFAULT_INFERENCE_SECS)),
        pull_secs=int(_parse_number(os.environ.get("FREECYCLE_PULL_TIMEOUT_SECS"), timeouts_dict.get("pullSecs") or DEFAULT_PULL_SECS)),
    )

    wol_dict = raw.get("wakeOnLan", {})
    wol = WakeOnLanConfig(
        enabled=_parse_boolean(os.environ.get("FREECYCLE_WOL_ENABLED"), wol_dict.get("enabled", DEFAULT_WOL_ENABLED)),
        mac_address=os.environ.get("FREECYCLE_WOL_MAC") or wol_dict.get("macAddress") or DEFAULT_WOL_MAC,
        broadcast_address=os.environ.get("FREECYCLE_WOL_BROADCAST") or wol_dict.get("broadcastAddress") or DEFAULT_WOL_BROADCAST,
        port=int(_parse_number(os.environ.get("FREECYCLE_WOL_PORT"), wol_dict.get("port", DEFAULT_WOL_PORT))),
        packet_count=int(_parse_number(os.environ.get("FREECYCLE_WOL_PACKET_COUNT"), wol_dict.get("packetCount") or DEFAULT_WOL_PACKET_COUNT)),
        packet_interval_secs=float(_parse_number(os.environ.get("FREECYCLE_WOL_PACKET_INTERVAL_SECS"), wol_dict.get("packetIntervalSecs") or DEFAULT_WOL_PACKET_INTERVAL_SECS)),
        poll_interval_secs=int(_parse_number(os.environ.get("FREECYCLE_WOL_POLL_INTERVAL_SECS"), wol_dict.get("pollIntervalSecs") or DEFAULT_WOL_POLL_INTERVAL_SECS)),
        max_wait_secs=int(_parse_number(os.environ.get("FREECYCLE_WOL_MAX_WAIT_SECS"), wol_dict.get("maxWaitSecs") or DEFAULT_WOL_MAX_WAIT_SECS)),
    )

    return McpServerConfig(servers=parsed_servers, engine=engine, timeouts=timeouts, wake_on_lan=wol)

def get_config_path() -> Path:
    """Get config file path, respecting env var override."""
    if override := os.environ.get("FREECYCLE_MCP_CONFIG"):
        return Path(override)
    return CONFIG_PATH

def get_config() -> McpServerConfig:
    """Get cached config, loading from file if needed."""
    global _config_cache
    if _config_cache is not None:
        return _config_cache

    raw = _read_raw_config()
    _config_cache = _merge_config(raw)

    if not _config_cache.servers:
        default_server = ServerEntry(
            host=DEFAULT_FC_HOST,
            port=DEFAULT_FC_PORT,
            approved=True,
        )
        _config_cache.servers = [default_server]

    return _config_cache

def reset_config_cache() -> None:
    """Clear config cache for reloading."""
    global _config_cache
    _config_cache = None

def get_active_server() -> ServerEntry:
    """Get first approved server or default."""
    cfg = get_config()
    for server in cfg.servers:
        if server.approved:
            return server
    if cfg.servers:
        return cfg.servers[0]
    return ServerEntry(host=DEFAULT_FC_HOST, port=DEFAULT_FC_PORT, approved=True)

def _server_entry_to_dict(server: ServerEntry) -> dict:
    """Convert ServerEntry to JSON-serializable dict."""
    d: dict[str, Any] = {
        "host": server.host,
        "port": server.port,
        "approved": server.approved,
    }
    if server.name:
        d["name"] = server.name
    if server.tls_fingerprint:
        d["tlsFingerprint"] = server.tls_fingerprint
    if server.identity_uuid:
        d["identityUuid"] = server.identity_uuid
    if server.wake_on_lan:
        d["wakeOnLan"] = {
            "enabled": server.wake_on_lan.enabled,
            "macAddress": server.wake_on_lan.mac_address,
            "broadcastAddress": server.wake_on_lan.broadcast_address,
            "port": server.wake_on_lan.port,
            "packetCount": server.wake_on_lan.packet_count,
            "packetIntervalSecs": server.wake_on_lan.packet_interval_secs,
            "pollIntervalSecs": server.wake_on_lan.poll_interval_secs,
            "maxWaitSecs": server.wake_on_lan.max_wait_secs,
        }
    if server.timeouts:
        d["timeouts"] = {
            "requestSecs": server.timeouts.request_secs,
            "inferenceSecs": server.timeouts.inference_secs,
            "pullSecs": server.timeouts.pull_secs,
        }
    return d

def _write_config_atomic(config_data: dict) -> None:
    """Write config atomically with Windows retry logic."""
    path = get_config_path()
    path.parent.mkdir(parents=True, exist_ok=True)

    max_retries = 3
    retry_delay_ms = 500

    for attempt in range(max_retries):
        try:
            with tempfile.NamedTemporaryFile(mode="w", suffix=".json", dir=path.parent, delete=False) as tmp:
                tmp_path = Path(tmp.name)
                json.dump(config_data, tmp, indent=2)
            os.replace(tmp_path, path)
            return
        except PermissionError as e:
            if attempt < max_retries - 1:
                time.sleep(retry_delay_ms / 1000.0)
            else:
                raise FreeCycleError(f"Failed to write config after {max_retries} attempts: {e}")
        except Exception as e:
            raise FreeCycleError(f"Failed to write config: {e}")

def save_config(patch: dict) -> None:
    """Save config with patch applied."""
    path = get_config_path()
    raw = _read_raw_config() if path.exists() else {}

    raw.update(patch)
    _write_config_atomic(raw)
    reset_config_cache()

# ============================================================================
# TLS / TOFU Secure Client
# ============================================================================

_protocol_cache: dict[str, tuple[str, float]] = {}

async def extract_server_fingerprint(host: str, port: int) -> str:
    """Extract SHA-256 fingerprint from server certificate."""
    def _do_extract():
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE

        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.settimeout(5)
            with ctx.wrap_socket(sock, server_hostname=host) as ssock:
                ssock.connect((host, port))
                cert_der = ssock.getpeercert(binary_form=True)
                if cert_der:
                    digest = hashlib.sha256(cert_der).hexdigest()
                    return digest
                raise FreeCycleError(f"No certificate from {host}:{port}")

    loop = asyncio.get_event_loop()
    return await loop.run_in_executor(None, _do_extract)

def verify_fingerprint(expected: str, actual: str) -> bool:
    """Compare fingerprints (case-insensitive)."""
    return expected.lower() == actual.lower()

async def secure_fetch(
    url: str,
    entry: Optional[ServerEntry] = None,
    method: str = "GET",
    body: Optional[str] = None,
    timeout_secs: Optional[int] = None,
) -> dict:
    """Fetch with TLS verification and protocol fallback."""
    if timeout_secs is None:
        timeout_secs = get_config().timeouts.request_secs

    parsed = urlparse(url)
    host = parsed.hostname or "localhost"
    port = parsed.port or 443
    scheme = parsed.scheme

    # Try HTTPS first if not explicitly HTTP
    if scheme != "http":
        try:
            ctx = None
            if entry:
                ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
                ctx.check_hostname = False
                ctx.verify_mode = ssl.CERT_NONE

                if entry.tls_fingerprint and entry.tls_fingerprint != "pending-verification":
                    actual_fp = await extract_server_fingerprint(host, port)
                    if not verify_fingerprint(entry.tls_fingerprint, actual_fp):
                        raise TLSFingerprintMismatchError(f"TLS fingerprint mismatch for {host}:{port}")

            https_url = f"https://{host}:{port}{parsed.path}"
            if parsed.query:
                https_url += f"?{parsed.query}"

            return await _request_response(https_url, method, body, timeout_secs, entry)
        except (TLSFingerprintMismatchError, FreeCycleConnectionError, FreeCycleTimeoutError):
            if entry and entry.tls_fingerprint:
                raise
            # Fall back to HTTP if not a known server

    # HTTP fallback
    http_url = f"http://{host}:{port if port != 443 else 80}{parsed.path}"
    if parsed.query:
        http_url += f"?{parsed.query}"

    return await _request_response(http_url, method, body, timeout_secs, entry)

# ============================================================================
# Async HTTP Client
# ============================================================================

_connection_pool: dict[str, http.client.HTTPConnection] = {}
_pool_lock = threading.Lock()

def _get_pool_key(url: str) -> str:
    """Generate pool key from URL."""
    parsed = urlparse(url)
    scheme = parsed.scheme or "http"
    host = parsed.hostname or "localhost"
    port = parsed.port or (443 if scheme == "https" else 80)
    return f"{scheme}://{host}:{port}"

def _raw_http_request(
    url: str,
    method: str = "GET",
    body: Optional[str] = None,
    timeout_secs: Optional[int] = None,
    ssl_context: Optional[ssl.SSLContext] = None,
) -> dict:
    """Synchronous HTTP request with connection pooling."""
    if timeout_secs is None:
        timeout_secs = 10

    parsed = urlparse(url)
    host = parsed.hostname or "localhost"
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    path = parsed.path or "/"
    if parsed.query:
        path += f"?{parsed.query}"

    is_https = parsed.scheme == "https"
    pool_key = _get_pool_key(url)

    def _do_request():
        conn = None
        try:
            with _pool_lock:
                conn = _connection_pool.pop(pool_key, None)

            if conn is None:
                if is_https:
                    conn = http.client.HTTPSConnection(host, port, timeout=timeout_secs, context=ssl_context)
                else:
                    conn = http.client.HTTPConnection(host, port, timeout=timeout_secs)

            headers = {"User-Agent": "FreeCycleClient/1.0.0"}
            if body:
                headers["Content-Type"] = "application/json"
                headers["Content-Length"] = str(len(body.encode()))

            try:
                conn.request(method, path, body=body, headers=headers)
                response = conn.getresponse()
                status_code = response.status
                response_body = response.read().decode("utf-8", errors="replace")

                with _pool_lock:
                    _connection_pool[pool_key] = conn

                return {
                    "status": status_code,
                    "body": response_body,
                }
            except (http.client.RemoteDisconnected, ConnectionResetError):
                conn.close()

                if is_https:
                    conn = http.client.HTTPSConnection(host, port, timeout=timeout_secs, context=ssl_context)
                else:
                    conn = http.client.HTTPConnection(host, port, timeout=timeout_secs)

                conn.request(method, path, body=body, headers=headers)
                response = conn.getresponse()
                status_code = response.status
                response_body = response.read().decode("utf-8", errors="replace")

                with _pool_lock:
                    _connection_pool[pool_key] = conn

                return {
                    "status": status_code,
                    "body": response_body,
                }
        except Exception as e:
            if conn:
                try:
                    conn.close()
                except:
                    pass
            raise e

    loop = asyncio.get_event_loop()
    return loop.run_in_executor(None, _do_request)

def _extract_response_message(parsed: dict, fallback: str) -> str:
    """Extract error message from response JSON."""
    if isinstance(parsed, dict):
        if "message" in parsed:
            return str(parsed["message"])
        if "error" in parsed:
            return str(parsed["error"])
    return fallback

async def _request_response(
    url: str,
    method: str = "GET",
    body: Optional[str] = None,
    timeout_secs: Optional[int] = None,
    server: Optional[ServerEntry] = None,
) -> dict:
    """Make HTTP request, return status/ok/body without raising on non-2xx."""
    try:
        ssl_ctx = None
        if server and url.startswith("https"):
            ssl_ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            ssl_ctx.check_hostname = False
            ssl_ctx.verify_mode = ssl.CERT_NONE

        result = await _raw_http_request(url, method, body, timeout_secs, ssl_ctx)
        status = result["status"]
        response_body = result["body"]

        try:
            parsed = json.loads(response_body)
        except json.JSONDecodeError:
            parsed = {"error": response_body[:300]}

        return {
            "status": status,
            "ok": 200 <= status < 300,
            "body": parsed,
        }
    except Exception as e:
        return {
            "status": 0,
            "ok": False,
            "body": {"error": str(e)},
        }

async def _request_json(
    url: str,
    method: str = "GET",
    body: Optional[str] = None,
    timeout_secs: Optional[int] = None,
    server: Optional[ServerEntry] = None,
) -> dict:
    """Make HTTP request, raise on non-2xx."""
    response = await _request_response(url, method, body, timeout_secs, server)

    if not response["ok"]:
        msg = _extract_response_message(response["body"], str(response["body"]))
        raise FreeCycleConnectionError(f"HTTP {response['status']} from {url}: {msg}")

    return response["body"]

# ============================================================================
# Wake-on-LAN
# ============================================================================

def normalize_mac_address(mac_address: str) -> bytes:
    """Normalize and convert MAC address to 6 bytes."""
    cleaned = "".join(c for c in mac_address if c in "0123456789abcdefABCDEF")
    if len(cleaned) != 12:
        raise ValueError(f"Invalid MAC address: {mac_address}")
    return bytes.fromhex(cleaned)

def create_magic_packet(mac_bytes: bytes) -> bytes:
    """Create WoL magic packet (102 bytes)."""
    if len(mac_bytes) != 6:
        raise ValueError("MAC address must be 6 bytes")
    return b'\xff' * 6 + mac_bytes * 16

def _send_wol_packet(packet: bytes, broadcast_address: str, port: int) -> None:
    """Send WoL packet via UDP broadcast."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.sendto(packet, (broadcast_address, port))
    finally:
        sock.close()

async def send_wake_on_lan_packets(config: WakeOnLanConfig) -> None:
    """Send WoL magic packets with graceful failure."""
    try:
        mac_bytes = normalize_mac_address(config.mac_address)
        packet = create_magic_packet(mac_bytes)

        loop = asyncio.get_event_loop()
        for sent in range(config.packet_count):
            await loop.run_in_executor(None, _send_wol_packet, packet, config.broadcast_address, config.port)
            if sent < config.packet_count - 1:
                await asyncio.sleep(config.packet_interval_secs)
    except (OSError, socket.error) as e:
        logger.warning(f"Wake-on-LAN failed: {e}")
    except ValueError as e:
        logger.warning(f"Wake-on-LAN configuration error: {e}")

# ============================================================================
# FreeCycle API Client (Port 7443)
# ============================================================================

def fc_resolve_base(server: Optional[ServerEntry] = None) -> str:
    """Resolve FreeCycle API base URL."""
    resolved = server or get_active_server()
    return f"https://{resolved.host}:{resolved.port}"

async def fc_health_check(server: Optional[ServerEntry] = None) -> dict:
    """Check health of the FreeCycle agent server.

    Sends a GET request to /health endpoint on the FreeCycle agent server.

    Args:
        server: Optional ServerEntry for routing. If None, uses active server.

    Returns:
        Health check response dict with ok flag and status.

    Raises:
        FreeCycleConnectionError: If connection fails.
        FreeCycleTimeoutError: If request times out.
    """
    resolved = server or get_active_server()
    base_url = f"https://{resolved.host}:{resolved.port}"
    return await _request_json(f"{base_url}/health", method="GET", server=server)

async def fc_get_status(server: Optional[ServerEntry] = None) -> dict:
    """Get FreeCycle status including VRAM, processes, and engine state.

    Args:
        server: Optional ServerEntry for routing.

    Returns:
        Status dict with engine running flag, VRAM info, blocking processes.
    """
    resolved = server or get_active_server()
    base_url = f"https://{resolved.host}:{resolved.port}"
    return await _request_json(f"{base_url}/status", method="GET", server=server)

async def fc_start_task_detailed(task_id: str, description: str, server: Optional[ServerEntry] = None) -> dict:
    """Signal task start, return full HTTP response.

    Args:
        task_id: Unique task identifier.
        description: Human-readable task description.
        server: Optional ServerEntry for routing.

    Returns:
        Dict with status, ok, and body fields (no exception on non-2xx).
    """
    resolved = server or get_active_server()
    base_url = f"https://{resolved.host}:{resolved.port}"
    body = json.dumps({"task_id": task_id, "description": description})
    return await _request_response(f"{base_url}/task/start", method="POST", body=body, server=server)

async def fc_start_task(task_id: str, description: str, server: Optional[ServerEntry] = None) -> dict:
    """Signal task start, raise on error.

    Args:
        task_id: Unique task identifier.
        description: Human-readable task description.
        server: Optional ServerEntry for routing.

    Returns:
        FreeCycle response dict.

    Raises:
        FreeCycleConnectionError: On HTTP error.
    """
    response = await fc_start_task_detailed(task_id, description, server)
    if not response["ok"]:
        msg = _extract_response_message(response["body"], str(response["body"]))
        raise FreeCycleConnectionError(f"HTTP {response['status']}: {msg}")
    return response["body"]

async def fc_stop_task_detailed(task_id: str, server: Optional[ServerEntry] = None) -> dict:
    """Signal task stop, return full HTTP response.

    Args:
        task_id: Task identifier to stop.
        server: Optional ServerEntry for routing.

    Returns:
        Dict with status, ok, and body fields (no exception on non-2xx).
    """
    resolved = server or get_active_server()
    base_url = f"https://{resolved.host}:{resolved.port}"
    body = json.dumps({"task_id": task_id})
    return await _request_response(f"{base_url}/task/stop", method="POST", body=body, server=server)

async def fc_stop_task(task_id: str, server: Optional[ServerEntry] = None) -> dict:
    """Signal task stop, raise on error.

    Args:
        task_id: Task identifier to stop.
        server: Optional ServerEntry for routing.

    Returns:
        FreeCycle response dict.

    Raises:
        FreeCycleConnectionError: On HTTP error.
    """
    response = await fc_stop_task_detailed(task_id, server)
    if not response["ok"]:
        msg = _extract_response_message(response["body"], str(response["body"]))
        raise FreeCycleConnectionError(f"HTTP {response['status']}: {msg}")
    return response["body"]

async def fc_install_model_detailed(model_name: str, server: Optional[ServerEntry] = None) -> dict:
    """Request model installation, return full HTTP response.

    Args:
        model_name: Model name to install.
        server: Optional ServerEntry for routing.

    Returns:
        Dict with status, ok, and body fields.
    """
    resolved = server or get_active_server()
    base_url = f"https://{resolved.host}:{resolved.port}"
    cfg = get_config()
    timeout_secs = cfg.timeouts.pull_secs
    body = json.dumps({"model_name": model_name})
    return await _request_response(f"{base_url}/models/install", method="POST", body=body, timeout_secs=timeout_secs, server=server)

async def fc_install_model(model_name: str, server: Optional[ServerEntry] = None) -> dict:
    """Request model installation, raise on error.

    Args:
        model_name: Model name to install.
        server: Optional ServerEntry for routing.

    Returns:
        FreeCycle response dict.

    Raises:
        FreeCycleConnectionError: On HTTP error.
    """
    response = await fc_install_model_detailed(model_name, server)
    if not response["ok"]:
        msg = _extract_response_message(response["body"], str(response["body"]))
        raise FreeCycleConnectionError(f"HTTP {response['status']}: {msg}")
    return response["body"]

async def fc_get_model_catalog(server: Optional[ServerEntry] = None) -> dict:
    """Fetch the model catalog from FreeCycle.

    Args:
        server: Optional ServerEntry for routing.

    Returns:
        Model catalog dict with models list and metadata.
    """
    resolved = server or get_active_server()
    base_url = f"https://{resolved.host}:{resolved.port}"
    return await _request_json(f"{base_url}/models/catalog", method="GET", server=server)

# ============================================================================
# Engine API Client (Port 11434)
# ============================================================================

def engine_resolve_base(server: Optional[ServerEntry] = None) -> str:
    """Resolve engine API base URL with FreeCycle proxy routing.

    Routes through HTTPS FreeCycle proxy when server port differs from configured
    engine port (indicating a FreeCycle server). Otherwise uses direct HTTP connection.

    Args:
        server: Optional ServerEntry for routing.

    Returns:
        Base URL string for engine API endpoints.
    """
    config = get_config()
    resolved = server or get_active_server()

    if resolved and resolved.port != config.engine.port:
        return f"https://{resolved.host}:{resolved.port}"
    return f"http://{config.engine.host}:{config.engine.port}"

async def engine_health_check(server: Optional[ServerEntry] = None) -> str:
    """Quick connectivity check via GET /api/version.

    Args:
        server: Optional ServerEntry for routing.

    Returns:
        Engine version string.

    Raises:
        FreeCycleConnectionError: If check fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()
    return await _request_json(f"{base}/api/version", method="GET", timeout_secs=cfg.timeouts.request_secs, server=server)

async def engine_generate(
    model: str,
    prompt: str,
    system_prompt: Optional[str] = None,
    temperature: Optional[float] = None,
    num_predict: Optional[int] = None,
    server: Optional[ServerEntry] = None,
) -> dict:
    """Send text generation request.

    Args:
        model: Model name for generation.
        prompt: Input prompt text.
        system_prompt: Optional system prompt.
        temperature: Sampling temperature (0-2).
        num_predict: Maximum tokens to generate.
        server: Optional ServerEntry for routing.

    Returns:
        Generation response with text, token counts, and timings.

    Raises:
        FreeCycleConnectionError: If request fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()

    body_dict: dict[str, Any] = {
        "model": model,
        "prompt": prompt,
        "stream": False,
    }
    if system_prompt:
        body_dict["system"] = system_prompt

    options: dict[str, Any] = {}
    if temperature is not None:
        options["temperature"] = temperature
    if num_predict is not None:
        options["num_predict"] = num_predict
    if options:
        body_dict["options"] = options

    body = json.dumps(body_dict)
    return await _request_json(
        f"{base}/api/generate",
        method="POST",
        body=body,
        timeout_secs=cfg.timeouts.inference_secs,
        server=server,
    )

async def engine_chat(
    model: str,
    messages: list[dict],
    system_prompt: Optional[str] = None,
    temperature: Optional[float] = None,
    num_predict: Optional[int] = None,
    server: Optional[ServerEntry] = None,
) -> dict:
    """Send chat completion request.

    Args:
        model: Model name for chat.
        messages: List of message dicts with role/content.
        system_prompt: Optional system prompt (prepended to messages).
        temperature: Sampling temperature.
        num_predict: Maximum tokens.
        server: Optional ServerEntry for routing.

    Returns:
        Chat response with message, token counts, and timings.

    Raises:
        FreeCycleConnectionError: If request fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()

    all_messages: list[dict] = []
    if system_prompt:
        all_messages.append({"role": "system", "content": system_prompt})
    all_messages.extend(messages)

    body_dict: dict[str, Any] = {
        "model": model,
        "messages": all_messages,
        "stream": False,
    }

    options: dict[str, Any] = {}
    if temperature is not None:
        options["temperature"] = temperature
    if num_predict is not None:
        options["num_predict"] = num_predict
    if options:
        body_dict["options"] = options

    body = json.dumps(body_dict)
    return await _request_json(
        f"{base}/api/chat",
        method="POST",
        body=body,
        timeout_secs=cfg.timeouts.inference_secs,
        server=server,
    )

async def engine_embed(
    model: str,
    input: Union[str, list[str]],
    server: Optional[ServerEntry] = None,
) -> dict:
    """Generate embeddings.

    Args:
        model: Embedding model name.
        input: Single string or list of strings to embed.
        server: Optional ServerEntry for routing.

    Returns:
        Embeddings response with embedding vectors.

    Raises:
        FreeCycleConnectionError: If request fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()

    body_dict: dict[str, Any] = {"model": model, "input": input}
    body = json.dumps(body_dict)
    return await _request_json(
        f"{base}/api/embed",
        method="POST",
        body=body,
        timeout_secs=cfg.timeouts.inference_secs,
        server=server,
    )

async def engine_list_models(server: Optional[ServerEntry] = None) -> dict:
    """List all locally available models.

    Args:
        server: Optional ServerEntry for routing.

    Returns:
        Dict with models list.

    Raises:
        FreeCycleConnectionError: If request fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()
    return await _request_json(
        f"{base}/api/tags",
        method="GET",
        timeout_secs=cfg.timeouts.request_secs,
        server=server,
    )

async def engine_show_model(name: str, server: Optional[ServerEntry] = None) -> dict:
    """Get detailed information about a model.

    Args:
        name: Model name.
        server: Optional ServerEntry for routing.

    Returns:
        Model details dict.

    Raises:
        FreeCycleConnectionError: If request fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()
    body = json.dumps({"model": name})
    return await _request_json(
        f"{base}/api/show",
        method="POST",
        body=body,
        timeout_secs=cfg.timeouts.request_secs,
        server=server,
    )

async def engine_pull_model(name: str, server: Optional[ServerEntry] = None) -> dict:
    """Download/pull a model.

    Note: Large models may exceed the default 10-minute timeout.
    Consider increasing timeouts.pull_secs in the config for very large models.

    Args:
        name: Model name to pull.
        server: Optional ServerEntry for routing.

    Returns:
        Pull response with status and progress.

    Raises:
        FreeCycleConnectionError: If request fails.
    """
    base = engine_resolve_base(server)
    cfg = get_config()
    body = json.dumps({"name": name, "stream": False})
    return await _request_json(
        f"{base}/api/pull",
        method="POST",
        body=body,
        timeout_secs=cfg.timeouts.pull_secs,
        server=server,
    )

# ============================================================================
# Multi-Server Router
# ============================================================================

_model_cache: dict[str, tuple[list[str], float]] = {}
_downed_servers: set[str] = set()

def _get_cache_key(server: ServerEntry) -> str:
    """Generate cache key for server."""
    return f"{server.host}:{server.port}"

def is_status_ready(status: dict) -> bool:
    """Check if server status indicates readiness.

    A server is ready if its status is "Available" or "Agent Task Active"
    AND the inference engine is running.

    Args:
        status: FreeCycle status dict from /status endpoint.

    Returns:
        True if server is ready for inference operations.
    """
    return (
        status.get("status") in ("Available", "Agent Task Active")
        and status.get("ollama_running", False)
    )

async def _query_server(server: ServerEntry) -> Union[ServerProbeResult, ServerProbeError]:
    """Query single server status."""
    cache_key = _get_cache_key(server)
    if cache_key in _downed_servers:
        return ServerProbeError(
            server=server,
            reachable=False,
            error="Server marked as down in this session",
        )

    try:
        status = await fc_get_status(server)
        free_vram_mb = status.get("vram_total_mb", 0) - status.get("vram_used_mb", 0)
        return ServerProbeResult(
            server=server,
            status=status,
            reachable=True,
            free_vram_mb=max(0, free_vram_mb),
        )
    except Exception as e:
        return ServerProbeError(
            server=server,
            reachable=False,
            error=str(e),
        )

async def query_all_servers() -> list[Union[ServerProbeResult, ServerProbeError]]:
    """Query all approved servers concurrently.

    Returns:
        List of probe results (preserves input order).
    """
    cfg = get_config()
    approved = [s for s in cfg.servers if s.approved]

    if not approved:
        return []

    results = await asyncio.gather(*[_query_server(s) for s in approved])
    return results

async def _get_models_for_server(server: ServerEntry) -> list[str]:
    """Get list of model names for server with caching."""
    cache_key = _get_cache_key(server)

    if cache_key in _model_cache:
        models, timestamp = _model_cache[cache_key]
        if time.time() - timestamp < MODEL_CACHE_TTL_SECS:
            return models

    try:
        response = await engine_list_models(server)
        model_names = [m.get("name", "") for m in response.get("models", [])]
        _model_cache[cache_key] = (model_names, time.time())
        return model_names
    except Exception:
        return []

async def select_best_server(model_name: Optional[str] = None) -> Optional[ServerProbeResult]:
    """Select best available server by free VRAM.

    If model_name provided, prefers servers that have the model.
    Returns None if no ready servers found (triggers cloud fallback).

    Args:
        model_name: Optional model name to prefer.

    Returns:
        Best ServerProbeResult or None for cloud fallback.
    """
    probes = await query_all_servers()

    ready: list[ServerProbeResult] = []
    for probe in probes:
        if isinstance(probe, ServerProbeResult) and is_status_ready(probe.status):
            ready.append(probe)

    if not ready:
        return None

    if not model_name:
        return max(ready, key=lambda p: p.free_vram_mb)

    has_model: list[ServerProbeResult] = []
    missing_model: list[ServerProbeResult] = []

    for server in ready:
        models = await _get_models_for_server(server.server)
        if model_name in models:
            has_model.append(server)
        else:
            missing_model.append(server)

    candidates = has_model if has_model else missing_model
    return max(candidates, key=lambda p: p.free_vram_mb) if candidates else None

def mark_server_down(server: ServerEntry) -> None:
    """Mark server as down for this session.

    Not persisted; cleared on process exit.

    Args:
        server: ServerEntry to mark down.
    """
    cache_key = _get_cache_key(server)
    _downed_servers.add(cache_key)

def clear_model_cache(server_key: Optional[str] = None) -> None:
    """Invalidate model cache.

    Use this when external state has changed (models pulled via CLI,
    new models available, etc.). Cache auto-expires after 5 minutes.

    Args:
        server_key: Optional "host:port" key to clear single server. If None, clears all.
    """
    if server_key:
        _model_cache.pop(server_key, None)
    else:
        _model_cache.clear()

# ============================================================================
# Availability Checker and Cloud Fallback
# ============================================================================

_pending_availability_check: Optional[asyncio.Future] = None

def _availability_result(
    available: bool = False,
    wake_on_lan_enabled: Optional[bool] = None,
    wake_on_lan_attempted: bool = False,
    freecycle_reachable: bool = False,
    engine_reachable: bool = False,
    freecycle_status: Optional[str] = None,
    blocking_processes: Optional[list[str]] = None,
    reason: str = "",
) -> LocalAvailability:
    """Build LocalAvailability result."""
    cfg = get_config()
    return LocalAvailability(
        available=available,
        wake_on_lan_enabled=wake_on_lan_enabled if wake_on_lan_enabled is not None else cfg.wake_on_lan.enabled,
        wake_on_lan_attempted=wake_on_lan_attempted,
        freecycle_reachable=freecycle_reachable,
        engine_reachable=engine_reachable,
        freecycle_status=freecycle_status,
        blocking_processes=blocking_processes or [],
        reason=reason,
    )

def _is_immediately_unavailable(status_str: str) -> bool:
    """Check if status indicates immediate fallback."""
    return status_str in IMMEDIATE_FALLBACK_STATUSES

async def _try_get_freecycle_status(server: Optional[ServerEntry] = None) -> dict:
    """Get FreeCycle status with error handling."""
    try:
        status = await fc_get_status(server)
        return {"ok": True, "status": status}
    except Exception as e:
        return {"ok": False, "message": str(e)}

async def _wait_for_availability(
    wake_on_lan_attempted: bool,
    initial_reason: str,
    server: Optional[ServerEntry] = None,
) -> LocalAvailability:
    """Poll for availability with deadline."""
    cfg = get_config()
    deadline = time.time() + cfg.wake_on_lan.max_wait_secs
    poll_interval = cfg.wake_on_lan.poll_interval_secs

    while time.time() < deadline:
        status_result = await _try_get_freecycle_status(server)

        if status_result["ok"]:
            status = status_result["status"]
            if is_status_ready(status):
                return _availability_result(
                    available=True,
                    freecycle_reachable=True,
                    engine_reachable=status.get("ollama_running", False),
                    freecycle_status=status.get("status"),
                    blocking_processes=status.get("blocking_processes", []),
                    reason="Local inference available",
                )

            if _is_immediately_unavailable(status.get("status", "")):
                return _availability_result(
                    available=False,
                    wake_on_lan_attempted=wake_on_lan_attempted,
                    freecycle_reachable=True,
                    freecycle_status=status.get("status"),
                    blocking_processes=status.get("blocking_processes", []),
                    reason=status.get("status", "Unavailable"),
                )

        await asyncio.sleep(poll_interval)

    return _availability_result(
        available=False,
        wake_on_lan_attempted=wake_on_lan_attempted,
        reason="Wake-on-LAN timeout",
    )

async def _perform_availability_check() -> LocalAvailability:
    """Perform full availability check."""
    cfg = get_config()

    try:
        await engine_health_check()
        return _availability_result(
            available=True,
            engine_reachable=True,
            reason="Engine responding",
        )
    except Exception:
        pass

    status_result = await _try_get_freecycle_status()

    if status_result["ok"]:
        status = status_result["status"]
        if is_status_ready(status):
            return _availability_result(
                available=True,
                freecycle_reachable=True,
                engine_reachable=status.get("ollama_running", False),
                freecycle_status=status.get("status"),
                blocking_processes=status.get("blocking_processes", []),
                reason="FreeCycle ready",
            )

        if _is_immediately_unavailable(status.get("status", "")):
            return _availability_result(
                available=False,
                freecycle_reachable=True,
                freecycle_status=status.get("status"),
                blocking_processes=status.get("blocking_processes", []),
                reason=status.get("status", "Unavailable"),
            )

    if not cfg.wake_on_lan.enabled:
        return _availability_result(
            available=False,
            freecycle_reachable=status_result["ok"],
            reason="Local inference unavailable",
        )

    if cfg.wake_on_lan.mac_address:
        await send_wake_on_lan_packets(cfg.wake_on_lan)

    return await _wait_for_availability(
        wake_on_lan_attempted=True,
        initial_reason="Waiting after WoL",
    )

async def ensure_local_availability() -> LocalAvailability:
    """Check local availability with deduplication.

    Multiple concurrent calls await the same in-flight check.
    Once complete, next call creates new future.

    Returns:
        LocalAvailability with readiness info and blocking processes.
    """
    global _pending_availability_check

    if _pending_availability_check is not None:
        try:
            return await _pending_availability_check
        except Exception:
            _pending_availability_check = None

    try:
        future: asyncio.Future[LocalAvailability] = asyncio.get_event_loop().create_future()
        _pending_availability_check = future

        result = await _perform_availability_check()
        future.set_result(result)
        return result
    except Exception as e:
        if _pending_availability_check:
            _pending_availability_check.set_exception(e)
        _pending_availability_check = None
        raise

def create_cloud_fallback_payload(action: str, availability: LocalAvailability) -> dict:
    """Create cloud fallback response payload.

    Args:
        action: Tool action name.
        availability: LocalAvailability result.

    Returns:
        Cloud fallback dict with all metadata.
    """
    return {
        "ok": False,
        "action": action,
        "local_available": False,
        "suggested_route": "cloud",
        "wake_on_lan_enabled": availability.wake_on_lan_enabled,
        "wake_on_lan_attempted": availability.wake_on_lan_attempted,
        "freecycle_reachable": availability.freecycle_reachable,
        "engine_reachable": availability.engine_reachable,
        "freecycle_status": availability.freecycle_status,
        "blocking_processes": availability.blocking_processes,
        "message": availability.reason,
    }

# ============================================================================
# Task Signaling
# ============================================================================

def sanitize_action(action: str) -> str:
    """Sanitize action name for task ID.

    Strips freecycle_ prefix, lowercases, replaces non-alnum with hyphens,
    trims leading/trailing hyphens.

    Args:
        action: Raw action name.

    Returns:
        Sanitized action string.
    """
    s = action.lower()
    if s.startswith("freecycle_"):
        s = s[10:]

    import re
    s = re.sub(r"[^a-z0-9-]", "-", s)
    s = re.sub(r"-+", "-", s)
    s = s.strip("-")
    return s

def build_task_id(action: str) -> str:
    """Build unique task ID.

    Format: mcp-{sanitized_action}-{timestamp_ms}-{uuid_8chars}

    Args:
        action: Action name.

    Returns:
        Task ID string.
    """
    sanitized = sanitize_action(action)
    timestamp_ms = int(time.time() * 1000)
    uuid_suffix = uuid.uuid4().hex[:8]
    return f"mcp-{sanitized}-{timestamp_ms}-{uuid_suffix}"

def validate_task_description(description: str) -> Optional[str]:
    """Validate task description against 4 constraints.

    Returns None if valid, error message if invalid.

    Constraints:
    1. Length 30-40 characters
    2. No single character dominance (>60%)
    3. At least one alphanumeric
    4. No word dominance (>60% same word)

    Args:
        description: Description to validate.

    Returns:
        None if valid, error message string if invalid.
    """
    if len(description) < 30 or len(description) > 40:
        return f"Description must be 30-40 chars (got {len(description)})"

    # Check character dominance
    char_counts: dict[str, int] = {}
    for ch in description:
        char_counts[ch] = char_counts.get(ch, 0) + 1
    max_count = max(char_counts.values()) if char_counts else 0
    if max_count > 0 and max_count / len(description) > 0.6:
        return "Description has character dominance >60%"

    # Check alphanumeric
    if not any(ch.isalnum() for ch in description):
        return "Description must contain at least one alphanumeric character"

    # Check word dominance
    words = description.split()
    qualifying_words = [w for w in words if len(w) >= 2]
    if len(qualifying_words) >= 3:
        word_counts: dict[str, int] = {}
        for word in qualifying_words:
            word_counts[word] = word_counts.get(word, 0) + 1
        max_word_count = max(word_counts.values()) if word_counts else 0
        if max_word_count > 0 and max_word_count / len(qualifying_words) > 0.6:
            return "Description has word dominance >60%"

    return None

def build_task_description(options: TrackedOperationOptions) -> str:
    """Build padded task description.

    Constructs from operation_label and model_name, trims to <=40 chars,
    pads to >=30 chars with "(local)" → "via API" → spaces strategy.
    Falls back to safe default on validation failure.

    Args:
        options: TrackedOperationOptions with label and model.

    Returns:
        Validated 30-40 char description.
    """
    base = options.operation_label
    if options.model_name:
        base += f" {options.model_name}"

    # Trim to <=40
    if len(base) > 40:
        base = base[:40].rsplit(" ", 1)[0] or base[:40]

    # Pad to >=30
    while len(base) < 30:
        if "(local)" not in base:
            candidate = base + " (local)"
            if len(candidate) <= 40:
                base = candidate
                continue

        if "via API" not in base:
            candidate = base + " via API"
            if len(candidate) <= 40:
                base = candidate
                continue

        base += " "

    # Validate and fallback
    if validate_task_description(base) is not None:
        return "MCP task via FreeCycle local API"

    return base

async def _build_conflict_payload(options: TrackedOperationOptions, message: str) -> dict:
    """Build conflict payload with optional status."""
    availability = _availability_result(
        available=False,
        freecycle_reachable=True,
        engine_reachable=False,
    )

    try:
        status_result = await _try_get_freecycle_status(options.server)
        if status_result["ok"]:
            status = status_result["status"]
            availability.blocking_processes = status.get("blocking_processes", [])
            availability.reason = status.get("status", message)
    except Exception:
        pass

    payload = create_cloud_fallback_payload(options.action, availability)
    payload["task_signal_conflict"] = True
    return payload

async def _begin_tracked_task(options: TrackedOperationOptions, task_id: str, description: str) -> Union[dict, str]:
    """Begin task tracking and handle response codes.

    Returns detailed response dict on success or error string on non-ok.
    """
    response = await fc_start_task_detailed(task_id, description, options.server)
    status = response["status"]
    body = response["body"]

    if response["ok"] and status == 200:
        return body

    if status == 400:
        logger.warning(f"Task start returned 400 (bad request): {body}")
        return body

    if status == 409:
        msg = _extract_response_message(body, "Task conflict")
        return await _build_conflict_payload(options, msg)

    if status >= 500:
        logger.warning(f"Task start returned {status}")
        return body

    raise FreeCycleConnectionError(f"Task start failed: HTTP {status}")

async def _stop_tracked_task(action: str, task_id: str, server: Optional[ServerEntry] = None) -> None:
    """Stop task tracking with graceful error handling."""
    try:
        response = await fc_stop_task_detailed(task_id, server)
        if not response["ok"]:
            if response["status"] == 404:
                logger.warning(f"Task {task_id} not found on server (drift)")
            else:
                logger.warning(f"Task stop returned {response['status']}")
    except Exception as e:
        logger.warning(f"Task stop failed: {e}")

async def run_tracked_local_operation(
    options: TrackedOperationOptions,
    operation: Callable[[], Awaitable[dict]],
) -> dict:
    """Run operation with automatic task lifecycle tracking.

    Returns {"kind": "completed", "value": result} or
            {"kind": "unavailable", "payload": cloud_fallback_dict}.

    Args:
        options: TrackedOperationOptions with action/label/model.
        operation: Async callable returning result dict.

    Returns:
        Result dict with kind and value/payload.
    """
    task_id = build_task_id(options.action)
    description = build_task_description(options)

    task_response = await _begin_tracked_task(options, task_id, description)

    if isinstance(task_response, dict) and "task_signal_conflict" in task_response:
        return {"kind": "unavailable", "payload": task_response}

    try:
        result = await operation()
        return {"kind": "completed", "value": result}
    finally:
        await _stop_tracked_task(options.action, task_id, options.server)

def safe_tokens_per_second(eval_count: Optional[int], eval_duration_ns: Optional[int]) -> float:
    """Safely compute tokens per second.

    Args:
        eval_count: Number of tokens evaluated.
        eval_duration_ns: Duration in nanoseconds.

    Returns:
        Tokens per second (rounded to 2 decimals) or 0.0 if invalid.
    """
    if (eval_count or 0) <= 0 or (eval_duration_ns or 0) <= 0:
        return 0.0
    return round((eval_count / (eval_duration_ns / 1e9)) * 100) / 100

def filter_warm_results(results: list[dict]) -> list[dict]:
    """Filter results to warm iterations (load_duration_ms < 500).

    Args:
        results: List of benchmark result dicts.

    Returns:
        Filtered list of warm results.
    """
    return [r for r in results if r.get("load_duration_ms", 0) < 500]

# ============================================================================
# FreeCycleClient Class
# ============================================================================

class FreeCycleClient:
    """High-level async Python client for FreeCycle GPU server.

    Provides full access to FreeCycle API with auto-tracking, multi-server
    routing, availability checking, and cloud fallback on local unavailability.
    """

    def __init__(self, config_path: Optional[Union[str, Path]] = None):
        """Initialize client with optional config path override.

        If config_path is provided, it overrides the default location
        and the config cache is reset.

        Args:
            config_path: Optional path to freecycle-mcp.config.json.
        """
        if config_path:
            os.environ["FREECYCLE_MCP_CONFIG"] = str(config_path)
            reset_config_cache()

    async def status(self) -> dict:
        """Get complete FreeCycle status with availability check.

        Returns:
            Status dict with VRAM, engine state, blocking processes, and
            optionally server_selected/server_name for multi-server configs.
        """
        availability = await ensure_local_availability()
        if not availability.available:
            return create_cloud_fallback_payload("freecycle_status", availability)

        try:
            best = await select_best_server()
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        result = await fc_get_status(selected_server)

        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def health(self) -> dict:
        """Quick health check of FreeCycle and engine without availability gate.

        Reports what it finds regardless of availability check status.

        Returns:
            Dict with ok flag, freecycle status/response, engine response,
            and optionally server metadata for multi-server configs.
        """
        try:
            best = await select_best_server()
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        freecycle_reachable = True
        freecycle_health: Any = None
        try:
            freecycle_health = await fc_health_check(selected_server)
        except Exception as e:
            freecycle_reachable = False
            freecycle_health = {"ok": False, "message": str(e)}

        try:
            engine_health = await engine_health_check(selected_server)
        except Exception as e:
            engine_health = {"ok": False, "message": str(e)}

        result: dict[str, Any] = {
            "ok": freecycle_reachable,
            "freecycle": freecycle_health,
            "freecycle_reachable": freecycle_reachable,
            "engine": engine_health,
        }

        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def check_availability(self) -> dict:
        """Check local availability and optionally fetch status.

        Returns:
            Availability dict with reason, blocking processes, and optional
            FreeCycle status enrichment if available.
        """
        availability = await ensure_local_availability()

        result: dict[str, Any] = {
            "available": availability.available,
            "wake_on_lan_enabled": availability.wake_on_lan_enabled,
            "wake_on_lan_attempted": availability.wake_on_lan_attempted,
            "freecycle_reachable": availability.freecycle_reachable,
            "engine_reachable": availability.engine_reachable,
            "blocking_processes": availability.blocking_processes,
            "message": availability.reason,
        }

        if availability.freecycle_status:
            result["freecycle_status"] = availability.freecycle_status

        return result

    async def start_task(self, task_id: str, description: str) -> dict:
        """Signal that work is starting on GPU.

        Args:
            task_id: Unique task identifier.
            description: Human-readable description.

        Returns:
            Task start response or cloud fallback dict if unavailable.
        """
        readiness = await self._prepare_local_tool("freecycle_start_task")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server()
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        result = await fc_start_task(task_id, description, selected_server)

        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def stop_task(self, task_id: str) -> dict:
        """Signal that work has finished.

        Args:
            task_id: Task identifier from start_task.

        Returns:
            Task stop response or cloud fallback dict.
        """
        readiness = await self._prepare_local_tool("freecycle_stop_task")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server()
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        result = await fc_stop_task(task_id, selected_server)

        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def list_models(self) -> dict:
        """List downloaded models with formatted shape.

        Returns:
            Dict with count and models list (name, size_mb, modified_at, digest),
            or cloud fallback.
        """
        readiness = await self._prepare_local_tool("freecycle_list_models")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server()
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        response = await engine_list_models(selected_server)
        models = response.get("models", [])
        shaped = [
            {
                "name": m.get("name"),
                "size_mb": round(m.get("size", 0) / (1024 * 1024)),
                "modified_at": m.get("modified_at"),
                "digest": m.get("digest", "")[:12],
            }
            for m in models
        ]

        cfg = get_config()
        result = {"count": len(shaped), "models": shaped}
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def show_model(self, model_name: str) -> dict:
        """Get model details.

        Args:
            model_name: Model name.

        Returns:
            Model details dict or cloud fallback.
        """
        readiness = await self._prepare_local_tool("freecycle_show_model")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server()
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        result = await engine_show_model(model_name, selected_server)

        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def pull_model(self, model_name: str) -> dict:
        """Download/pull a model (auto-tracked).

        Args:
            model_name: Model name to pull.

        Returns:
            Pull response or cloud fallback dict.
        """
        availability = await ensure_local_availability()
        readiness = await self._prepare_local_tool("freecycle_pull_model")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server(model_name)
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        options = TrackedOperationOptions(
            action="freecycle_pull_model",
            operation_label="MCP pull",
            model_name=model_name,
            availability=availability,
            server=selected_server,
        )

        async def do_pull():
            return await engine_pull_model(model_name, selected_server)

        tracked = await run_tracked_local_operation(options, do_pull)
        if tracked["kind"] == "unavailable":
            return tracked["payload"]

        result = tracked["value"]
        clear_model_cache(_get_cache_key(selected_server)) if selected_server else clear_model_cache()

        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def generate(
        self,
        prompt: str,
        model: str = "llama3.1:8b-instruct-q4_K_M",
        system_prompt: Optional[str] = None,
        temperature: Optional[float] = None,
        num_predict: Optional[int] = None,
    ) -> dict:
        """Generate text (auto-tracked).

        Args:
            prompt: Input prompt.
            model: Model name (default: llama3.1:8b-instruct-q4_K_M).
            system_prompt: Optional system prompt.
            temperature: Sampling temperature.
            num_predict: Max tokens to generate.

        Returns:
            Generated text response or cloud fallback dict.
        """
        availability = await ensure_local_availability()
        readiness = await self._prepare_local_tool("freecycle_generate")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server(model)
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        options = TrackedOperationOptions(
            action="freecycle_generate",
            operation_label="MCP generate",
            model_name=model,
            availability=availability,
            server=selected_server,
        )

        async def do_generate():
            response = await engine_generate(
                model, prompt,
                system_prompt=system_prompt,
                temperature=temperature,
                num_predict=num_predict,
                server=selected_server,
            )
            return {
                "response": response.get("response"),
                "model": response.get("model"),
                "tokens_generated": response.get("eval_count") or 0,
                "tokens_per_second": safe_tokens_per_second(
                    response.get("eval_count"),
                    response.get("eval_duration"),
                ),
                "total_duration_ms": round(response.get("total_duration", 0) / 1e6) if response.get("total_duration") is not None else None,
            }

        tracked = await run_tracked_local_operation(options, do_generate)
        if tracked["kind"] == "unavailable":
            return tracked["payload"]

        result = tracked["value"]
        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def chat(
        self,
        messages: list[dict],
        model: str = "llama3.1:8b-instruct-q4_K_M",
        system_prompt: Optional[str] = None,
        temperature: Optional[float] = None,
        num_predict: Optional[int] = None,
    ) -> dict:
        """Chat completion (auto-tracked).

        Args:
            messages: List of message dicts with role/content.
            model: Model name (default: llama3.1:8b-instruct-q4_K_M).
            system_prompt: Optional system prompt.
            temperature: Sampling temperature.
            num_predict: Max tokens.

        Returns:
            Chat response or cloud fallback dict.
        """
        availability = await ensure_local_availability()
        readiness = await self._prepare_local_tool("freecycle_chat")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server(model)
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        options = TrackedOperationOptions(
            action="freecycle_chat",
            operation_label="MCP chat",
            model_name=model,
            availability=availability,
            server=selected_server,
        )

        async def do_chat():
            response = await engine_chat(
                model, messages,
                system_prompt=system_prompt,
                temperature=temperature,
                num_predict=num_predict,
                server=selected_server,
            )
            return {
                "message": response.get("message"),
                "model": response.get("model"),
                "tokens_generated": response.get("eval_count") or 0,
                "tokens_per_second": safe_tokens_per_second(
                    response.get("eval_count"),
                    response.get("eval_duration"),
                ),
                "total_duration_ms": round(response.get("total_duration", 0) / 1e6) if response.get("total_duration") is not None else None,
            }

        tracked = await run_tracked_local_operation(options, do_chat)
        if tracked["kind"] == "unavailable":
            return tracked["payload"]

        result = tracked["value"]
        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def embed(
        self,
        input: Union[str, list[str]],
        model: str = "nomic-embed-text",
    ) -> dict:
        """Generate embeddings (auto-tracked).

        Args:
            input: String or list of strings to embed.
            model: Embedding model (default: nomic-embed-text).

        Returns:
            Embeddings response or cloud fallback dict.
        """
        availability = await ensure_local_availability()
        readiness = await self._prepare_local_tool("freecycle_embed")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server(model)
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        options = TrackedOperationOptions(
            action="freecycle_embed",
            operation_label="MCP embed",
            model_name=model,
            availability=availability,
            server=selected_server,
        )

        async def do_embed():
            response = await engine_embed(input, model, selected_server)
            embeddings = response.get("embeddings", [])
            return {
                "model": response.get("model"),
                "embedding_count": len(embeddings),
                "dimensions": len(embeddings[0]) if embeddings else 0,
                "embeddings": embeddings,
                "total_duration_ms": round(response.get("total_duration", 0) / 1e6) if response.get("total_duration") is not None else None,
            }

        tracked = await run_tracked_local_operation(options, do_embed)
        if tracked["kind"] == "unavailable":
            return tracked["payload"]

        result = tracked["value"]
        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def evaluate_task(self, task_description: str, requirements: Optional[dict] = None) -> dict:
        """Evaluate task routing (local/cloud/hybrid).

        Args:
            task_description: Description of the task.
            requirements: Optional dict with latency/quality/cost/privacy keys.

        Returns:
            Routing recommendation dict with local_available flag.
        """
        availability = await ensure_local_availability()

        reqs = requirements or {}
        privacy_level = reqs.get("privacy", "normal")
        latency_level = reqs.get("latency", "normal")
        quality_level = reqs.get("quality", "normal")
        cost_level = reqs.get("cost", "normal")

        # Task classification
        lower_desc = task_description.lower()
        local_score = sum(1 for kw in LOCAL_KEYWORDS if kw in lower_desc)
        cloud_score = sum(1 for kw in CLOUD_KEYWORDS if kw in lower_desc)

        if cloud_score > local_score:
            task_class = "cloud"
        elif local_score > 0 and cloud_score == 0:
            task_class = "local"
        else:
            task_class = "hybrid"

        # Routing logic (matching Node.js exactly)
        if privacy_level == "critical":
            route = "local" if availability.available else "cloud"
            if not availability.available:
                reason = "Privacy-critical task requires local execution, but local unavailable"
            else:
                reason = "Privacy-critical task routed to local"
        elif not availability.available:
            route = "cloud"
            reason = "Local engine unavailable"
        elif latency_level == "low" and task_class != "cloud":
            route = "local"
            reason = "Low-latency requirement prefers local"
        elif quality_level == "high" and task_class == "cloud":
            route = "cloud"
            reason = "High-quality task prefers cloud"
        elif cost_level == "minimize" and task_class != "cloud":
            route = "local"
            reason = "Cost minimization prefers local"
        elif task_class == "local":
            route = "local"
            reason = "Task classification: local"
        elif task_class == "cloud":
            route = "cloud"
            reason = "Task classification: cloud"
        else:
            route = "hybrid"
            reason = "Task classification: hybrid"

        cfg = get_config()
        result = {
            "task_description": task_description,
            "suggested_route": route,
            "local_available": availability.available,
            "blocking_processes": availability.blocking_processes,
            "reason": reason,
            "wake_on_lan_enabled": availability.wake_on_lan_enabled,
        }

        try:
            best = await select_best_server()
            selected_server = best.server if best else None
            if len(cfg.servers) > 1 and selected_server:
                result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
                result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"
        except Exception:
            pass

        return result

    async def benchmark(self, model: str, prompt: str, iterations: int = 3) -> dict:
        """Run inference benchmark (auto-tracked).

        Args:
            model: Model to benchmark.
            prompt: Input prompt.
            iterations: Number of iterations (default: 3).

        Returns:
            Benchmark results dict or cloud fallback.
        """
        availability = await ensure_local_availability()
        readiness = await self._prepare_local_tool("freecycle_benchmark")
        if not readiness.available:
            return readiness.payload

        try:
            best = await select_best_server(model)
            selected_server = best.server if best else None
        except Exception:
            selected_server = None

        options = TrackedOperationOptions(
            action="freecycle_benchmark",
            operation_label="MCP benchmark",
            model_name=model,
            availability=availability,
            server=selected_server,
        )

        async def do_benchmark():
            results: list[dict] = []

            for i in range(iterations):
                started_at = time.time() * 1000
                response = await engine_generate(model, prompt, num_predict=100, server=selected_server)
                latency_ms = round(time.time() * 1000 - started_at)

                results.append({
                    "latency_ms": latency_ms,
                    "tokens": response.get("eval_count") or 0,
                    "tokens_per_sec": safe_tokens_per_second(
                        response.get("eval_count"),
                        response.get("eval_duration"),
                    ),
                    "load_duration_ms": round((response.get("load_duration") or 0) / 1e6),
                    "prompt_tokens": response.get("prompt_eval_count") or 0,
                    "prompt_eval_ms": round((response.get("prompt_eval_duration") or 0) / 1e6),
                    "generation_ms": round((response.get("eval_duration") or 0) / 1e6),
                    "engine_total_ms": round((response.get("total_duration") or 0) / 1e6),
                })

            avg_latency = round(sum(r["latency_ms"] for r in results) / len(results)) if results else 0
            avg_tps = round((sum(r["tokens_per_sec"] for r in results) / len(results)) * 100) / 100 if results else 0

            warm = filter_warm_results(results)
            warm_latency = round(sum(r["latency_ms"] for r in warm) / len(warm)) if warm else None
            warm_tps = round((sum(r["tokens_per_sec"] for r in warm) / len(warm)) * 100) / 100 if warm else None

            return {
                "model": model,
                "iterations": len(results),
                "results": results,
                "average_latency_ms": avg_latency,
                "average_tokens_per_second": avg_tps,
                "warm_average_latency_ms": warm_latency,
                "warm_average_tokens_per_second": warm_tps,
                "warm_iteration_count": len(warm),
            }

        tracked = await run_tracked_local_operation(options, do_benchmark)
        if tracked["kind"] == "unavailable":
            return tracked["payload"]

        result = tracked["value"]
        cfg = get_config()
        if len(cfg.servers) > 1 and selected_server:
            result["server_selected"] = f"{selected_server.host}:{selected_server.port}"
            result["server_name"] = selected_server.name or f"{selected_server.host}:{selected_server.port}"

        return result

    async def add_server(self, ip: str, port: int = 7443, name: Optional[str] = None) -> dict:
        """Add a new FreeCycle server to config.

        Probes server via TLS, extracts fingerprint, and persists to config
        with approved=False (user must approve).

        Args:
            ip: IP address of server.
            port: Port number (default: 7443).
            name: Optional friendly name.

        Returns:
            Response dict with ok flag and result/error message.
        """
        try:
            # Try TLS first
            try:
                fingerprint = await extract_server_fingerprint(ip, port)
                probe_entry = ServerEntry(
                    host=ip, port=port, approved=False, tls_fingerprint=fingerprint
                )
                await fc_health_check(probe_entry)

                # Success with TLS
                cfg = get_config()
                new_entry = ServerEntry(
                    host=ip,
                    port=port,
                    name=name,
                    approved=False,
                    tls_fingerprint=fingerprint,
                )

                servers_list = [_server_entry_to_dict(s) for s in cfg.servers]
                servers_list.append(_server_entry_to_dict(new_entry))
                save_config({"servers": servers_list})

                return {
                    "ok": True,
                    "message": f"Server {ip}:{port} added (requires approval)",
                    "server": f"{ip}:{port}",
                    "tls_fingerprint": fingerprint,
                }
            except (FreeCycleConnectionError, TLSFingerprintMismatchError):
                # Try HTTP fallback
                probe_entry = ServerEntry(host=ip, port=port, approved=False)
                await fc_health_check(probe_entry)

                cfg = get_config()
                new_entry = ServerEntry(
                    host=ip,
                    port=port,
                    name=name,
                    approved=False,
                )

                servers_list = [_server_entry_to_dict(s) for s in cfg.servers]
                servers_list.append(_server_entry_to_dict(new_entry))
                save_config({"servers": servers_list})

                return {
                    "ok": True,
                    "message": f"Server {ip}:{port} added via HTTP (requires approval)",
                    "server": f"{ip}:{port}",
                }
        except Exception as e:
            return {
                "ok": False,
                "message": f"Failed to add server: {e}",
            }

    async def list_servers(self) -> dict:
        """List configured servers with current status.

        Queries all approved servers and cross-references with config.

        Returns:
            Dict with servers list including status/VRAM/reachability.
        """
        probes = await query_all_servers()
        probe_map: dict[str, Union[ServerProbeResult, ServerProbeError]] = {}
        for probe in probes:
            key = f"{probe.server.host}:{probe.server.port}"
            probe_map[key] = probe

        cfg = get_config()
        servers_list: list[dict] = []

        for server in cfg.servers:
            key = f"{server.host}:{server.port}"
            probe = probe_map.get(key)

            server_dict: dict[str, Any] = {
                "host": server.host,
                "port": server.port,
                "name": server.name or f"{server.host}:{server.port}",
                "approved": server.approved,
                "reachable": False,
            }

            if isinstance(probe, ServerProbeResult):
                server_dict["reachable"] = True
                server_dict["status"] = probe.status.get("status")
                server_dict["vram_free_mb"] = probe.free_vram_mb
                server_dict["vram_total_mb"] = probe.status.get("vram_total_mb")
                server_dict["vram_used_mb"] = probe.status.get("vram_used_mb")
                server_dict["engine_running"] = probe.status.get("ollama_running", False)
            elif isinstance(probe, ServerProbeError):
                server_dict["error"] = probe.error

            servers_list.append(server_dict)

        return {
            "servers": servers_list,
            "count": len(servers_list),
        }

    async def model_catalog(self) -> dict:
        """Fetch the model catalog without availability check.

        Returns:
            Catalog dict with models list and metadata, or error dict.
        """
        try:
            try:
                best = await select_best_server()
                selected_server = best.server if best else None
            except Exception:
                selected_server = None

            catalog = await fc_get_model_catalog(selected_server)
            return {
                "ok": True,
                "catalog": catalog,
                "total_models": len(catalog.get("models", [])),
                "synthesized": catalog.get("synthesized"),
                "scraped_at": catalog.get("scraped_at"),
            }
        except Exception as e:
            return {
                "ok": False,
                "error": str(e),
                "message": "Failed to fetch model catalog",
            }

    def invalidate_cache(self, server_key: Optional[str] = None) -> None:
        """Invalidate the cached model list for one or all servers.

        Use this when you know external state has changed -- for example,
        if a model was pulled directly via the engine CLI or another client
        bypassing this module. The cache automatically expires after 5 minutes,
        but this method forces an immediate refresh on the next model query.

        Calling without arguments clears the entire cache. Passing a server_key
        (formatted as "host:port") clears only that server's cached model list.

        Args:
            server_key: Optional server identifier in "host:port" format.
                If None, all cached model lists are cleared.

        Example:
            # After pulling a model directly via the engine CLI:
            client.invalidate_cache()

            # Clear cache for a specific server only:
            client.invalidate_cache("192.168.1.100:7443")
        """
        clear_model_cache(server_key)

    # Private helper methods

    async def _prepare_local_tool(self, action: str) -> dict:
        """Prepare tool by checking availability.

        Returns:
            Dict with available flag and optional payload for cloud fallback.
        """
        availability = await ensure_local_availability()
        if not availability.available:
            return {
                "available": False,
                "payload": create_cloud_fallback_payload(action, availability),
            }
        return {"available": True}

    # Sync wrappers

    def _run_sync(self, coro: Awaitable[Any]) -> Any:
        """Run async coroutine from sync context with loop detection.

        Detects if already in an event loop (Jupyter, async framework) and
        creates a new thread to avoid "This event loop is already running" error.
        """
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = None

        if loop is not None:
            # Already in async context - use thread
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                return pool.submit(asyncio.run, coro).result()
        else:
            return asyncio.run(coro)

    def status_sync(self) -> dict:
        """Sync wrapper for status()."""
        return self._run_sync(self.status())

    def health_sync(self) -> dict:
        """Sync wrapper for health()."""
        return self._run_sync(self.health())

    def check_availability_sync(self) -> dict:
        """Sync wrapper for check_availability()."""
        return self._run_sync(self.check_availability())

    def start_task_sync(self, task_id: str, description: str) -> dict:
        """Sync wrapper for start_task()."""
        return self._run_sync(self.start_task(task_id, description))

    def stop_task_sync(self, task_id: str) -> dict:
        """Sync wrapper for stop_task()."""
        return self._run_sync(self.stop_task(task_id))

    def list_models_sync(self) -> dict:
        """Sync wrapper for list_models()."""
        return self._run_sync(self.list_models())

    def show_model_sync(self, model_name: str) -> dict:
        """Sync wrapper for show_model()."""
        return self._run_sync(self.show_model(model_name))

    def pull_model_sync(self, model_name: str) -> dict:
        """Sync wrapper for pull_model()."""
        return self._run_sync(self.pull_model(model_name))

    def generate_sync(
        self,
        prompt: str,
        model: str = "llama3.1:8b-instruct-q4_K_M",
        system_prompt: Optional[str] = None,
        temperature: Optional[float] = None,
        num_predict: Optional[int] = None,
    ) -> dict:
        """Sync wrapper for generate()."""
        return self._run_sync(
            self.generate(prompt, model, system_prompt, temperature, num_predict)
        )

    def chat_sync(
        self,
        messages: list[dict],
        model: str = "llama3.1:8b-instruct-q4_K_M",
        system_prompt: Optional[str] = None,
        temperature: Optional[float] = None,
        num_predict: Optional[int] = None,
    ) -> dict:
        """Sync wrapper for chat()."""
        return self._run_sync(
            self.chat(messages, model, system_prompt, temperature, num_predict)
        )

    def embed_sync(
        self,
        input: Union[str, list[str]],
        model: str = "nomic-embed-text",
    ) -> dict:
        """Sync wrapper for embed()."""
        return self._run_sync(self.embed(input, model))

    def evaluate_task_sync(
        self,
        task_description: str,
        requirements: Optional[dict] = None,
    ) -> dict:
        """Sync wrapper for evaluate_task()."""
        return self._run_sync(self.evaluate_task(task_description, requirements))

    def benchmark_sync(self, model: str, prompt: str, iterations: int = 3) -> dict:
        """Sync wrapper for benchmark()."""
        return self._run_sync(self.benchmark(model, prompt, iterations))

    def add_server_sync(
        self,
        ip: str,
        port: int = 7443,
        name: Optional[str] = None,
    ) -> dict:
        """Sync wrapper for add_server()."""
        return self._run_sync(self.add_server(ip, port, name))

    def list_servers_sync(self) -> dict:
        """Sync wrapper for list_servers()."""
        return self._run_sync(self.list_servers())

    def model_catalog_sync(self) -> dict:
        """Sync wrapper for model_catalog()."""
        return self._run_sync(self.model_catalog())

# ============================================================================
# CLI Entry Point
# ============================================================================

def main() -> None:
    """Command-line interface for FreeCycle client."""
    parser = argparse.ArgumentParser(
        description="FreeCycle GPU Lifecycle Manager Python Client",
        prog="freecycle_client",
    )

    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    parser.add_argument("--config", help="Path to config file", default=None)
    parser.add_argument("--pretty", action="store_true", help="Human-readable output")

    subparsers = parser.add_subparsers(dest="command", help="Command to run")

    # Status command
    subparsers.add_parser("status", help="Get full FreeCycle status")
    subparsers.add_parser("health", help="Quick health check")
    subparsers.add_parser("check", help="Check local availability")

    # Model commands
    subparsers.add_parser("list-models", help="List downloaded models")
    show_parser = subparsers.add_parser("show-model", help="Show model details")
    show_parser.add_argument("model", help="Model name")
    pull_parser = subparsers.add_parser("pull-model", help="Download a model")
    pull_parser.add_argument("model", help="Model name to pull")

    # Inference commands
    gen_parser = subparsers.add_parser("generate", help="Text generation")
    gen_parser.add_argument("--prompt", required=True, help="Input prompt")
    gen_parser.add_argument("--model", default="llama3.1:8b-instruct-q4_K_M", help="Model to use")
    gen_parser.add_argument("--system", help="System prompt")
    gen_parser.add_argument("--temperature", type=float, help="Sampling temperature")
    gen_parser.add_argument("--num-predict", type=int, help="Max tokens")

    chat_parser = subparsers.add_parser("chat", help="Chat completion")
    chat_parser.add_argument("--messages", required=True, help="JSON array of message objects")
    chat_parser.add_argument("--model", default="llama3.1:8b-instruct-q4_K_M", help="Model to use")
    chat_parser.add_argument("--system", help="System prompt")
    chat_parser.add_argument("--temperature", type=float, help="Sampling temperature")
    chat_parser.add_argument("--num-predict", type=int, help="Max tokens")

    embed_parser = subparsers.add_parser("embed", help="Generate embeddings")
    embed_parser.add_argument("--input", required=True, help="Text or JSON array of texts")
    embed_parser.add_argument("--model", default="nomic-embed-text", help="Embedding model")

    # Evaluation and benchmarking
    eval_parser = subparsers.add_parser("evaluate", help="Evaluate task routing")
    eval_parser.add_argument("description", help="Task description")
    eval_parser.add_argument("--requirements", help="JSON requirements dict")

    bench_parser = subparsers.add_parser("benchmark", help="Run inference benchmark")
    bench_parser.add_argument("--model", required=True, help="Model to benchmark")
    bench_parser.add_argument("--prompt", required=True, help="Input prompt")
    bench_parser.add_argument("--iterations", type=int, default=3, help="Number of iterations")

    # Server management
    subparsers.add_parser("list-servers", help="List configured servers")
    add_parser = subparsers.add_parser("add-server", help="Add a new server")
    add_parser.add_argument("ip", help="Server IP address")
    add_parser.add_argument("--port", type=int, default=7443, help="Server port")
    add_parser.add_argument("--name", help="Server name")

    # Task lifecycle
    start_parser = subparsers.add_parser("start-task", help="Signal task start")
    start_parser.add_argument("task_id", help="Task ID")
    start_parser.add_argument("description", help="Task description")
    stop_parser = subparsers.add_parser("stop-task", help="Signal task stop")
    stop_parser.add_argument("task_id", help="Task ID")

    # Catalog
    subparsers.add_parser("catalog", help="Fetch model catalog")

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        sys.exit(0)

    # Set up logging
    logging.basicConfig(level=logging.INFO)

    # Create client
    client = FreeCycleClient(config_path=args.config)

    # Run command
    try:
        result = None

        if args.command == "status":
            result = client.status_sync()
        elif args.command == "health":
            result = client.health_sync()
        elif args.command == "check":
            result = client.check_availability_sync()
        elif args.command == "list-models":
            result = client.list_models_sync()
        elif args.command == "show-model":
            result = client.show_model_sync(args.model)
        elif args.command == "pull-model":
            result = client.pull_model_sync(args.model)
        elif args.command == "generate":
            result = client.generate_sync(
                prompt=args.prompt,
                model=args.model,
                system_prompt=args.system,
                temperature=args.temperature,
                num_predict=args.num_predict,
            )
        elif args.command == "chat":
            messages = json.loads(args.messages)
            result = client.chat_sync(
                messages=messages,
                model=args.model,
                system_prompt=args.system,
                temperature=args.temperature,
                num_predict=args.num_predict,
            )
        elif args.command == "embed":
            try:
                input_data = json.loads(args.input)
            except json.JSONDecodeError:
                input_data = args.input
            result = client.embed_sync(input=input_data, model=args.model)
        elif args.command == "evaluate":
            requirements = None
            if args.requirements:
                requirements = json.loads(args.requirements)
            result = client.evaluate_task_sync(args.description, requirements)
        elif args.command == "benchmark":
            result = client.benchmark_sync(
                model=args.model,
                prompt=args.prompt,
                iterations=args.iterations,
            )
        elif args.command == "list-servers":
            result = client.list_servers_sync()
        elif args.command == "add-server":
            result = client.add_server_sync(args.ip, args.port, args.name)
        elif args.command == "start-task":
            result = client.start_task_sync(args.task_id, args.description)
        elif args.command == "stop-task":
            result = client.stop_task_sync(args.task_id)
        elif args.command == "catalog":
            result = client.model_catalog_sync()

        if result is not None:
            if args.pretty:
                # Pretty print
                if isinstance(result, dict):
                    for key, value in result.items():
                        print(f"{key}: {value}")
                else:
                    print(result)
            else:
                # JSON output
                print(json.dumps(result, indent=2))
    except Exception as e:
        logger.error(f"Error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
