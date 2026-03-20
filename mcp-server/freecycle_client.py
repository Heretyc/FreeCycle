"""FreeCycle GPU Lifecycle Manager — Python Client Library.

A companion module to the Node.js MCP server that provides direct Python access
to the FreeCycle GPU server API, enabling agentic workflows to bypass MCP protocol
overhead and write directly to the local inference engine.

Supports both async (primary) and sync (convenience wrapper) interfaces.

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
import hashlib
import http.client
import json
import logging
import os
import socket
import ssl
import sys
import tempfile
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional, Union
from urllib.parse import urlparse

__version__ = "1.0.0"

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
    """Base exception for all FreeCycle-related errors."""

    pass


class FreeCycleConnectionError(FreeCycleError):
    """Raised when a connection to FreeCycle or the engine fails."""

    pass


class FreeCycleTimeoutError(FreeCycleError):
    """Raised when a request to FreeCycle or the engine times out."""

    pass


class FreeCycleUnavailableError(FreeCycleError):
    """Raised when FreeCycle or the engine is not available."""

    pass


class TLSFingerprintMismatchError(FreeCycleError):
    """Raised when a server's TLS fingerprint does not match the expected value."""

    pass


class TaskConflictError(FreeCycleError):
    """Raised when a task cannot be started due to a conflict (e.g., task already in progress)."""

    pass


class ConfigError(FreeCycleError):
    """Raised when there is an error reading, parsing, or writing the config file."""

    pass


# ============================================================================
# Dataclasses — Configuration Types
# ============================================================================

@dataclass
class EndpointConfig:
    """Configuration for a network endpoint.

    Attributes:
        host: The hostname or IP address.
        port: The port number.
    """

    host: str
    port: int


@dataclass
class TimeoutConfig:
    """Configuration for various timeout intervals.

    Attributes:
        request_secs: Timeout for regular requests (default 10s).
        inference_secs: Timeout for inference operations (default 300s / 5 min).
        pull_secs: Timeout for model pull operations (default 600s / 10 min).
    """

    request_secs: float
    inference_secs: float
    pull_secs: float


@dataclass
class WakeOnLanConfig:
    """Configuration for Wake-on-LAN magic packet broadcasting.

    Attributes:
        enabled: Whether Wake-on-LAN is enabled.
        mac_address: Target MAC address (format: AA:BB:CC:DD:EE:FF).
        broadcast_address: Broadcast address for magic packets (default 255.255.255.255).
        port: UDP port for magic packets (default 9).
        packet_count: Number of magic packets to send (default 5).
        packet_interval_secs: Delay between packets in seconds (default 0.25s).
        poll_interval_secs: Polling interval while waiting for server (default 30s).
        max_wait_secs: Maximum wait time for server to come online (default 900s / 15 min).
    """

    enabled: bool
    mac_address: str
    broadcast_address: str
    port: int
    packet_count: int
    packet_interval_secs: float
    poll_interval_secs: float
    max_wait_secs: float


@dataclass
class ServerEntry:
    """Configuration for a single FreeCycle server.

    Attributes:
        host: Hostname or IP address of the FreeCycle server.
        port: Port number of the FreeCycle server.
        approved: Whether this server is approved for use (default True).
        name: Optional friendly name for the server.
        tls_fingerprint: Optional SHA-256 TLS fingerprint for TOFU verification.
        identity_uuid: Optional UUID identifying the server instance.
        wake_on_lan: Optional per-server Wake-on-LAN configuration override.
        timeouts: Optional per-server timeout configuration override.
    """

    host: str
    port: int
    approved: bool = True
    name: Optional[str] = None
    tls_fingerprint: Optional[str] = None
    identity_uuid: Optional[str] = None
    wake_on_lan: Optional[WakeOnLanConfig] = None
    timeouts: Optional[TimeoutConfig] = None


@dataclass
class McpServerConfig:
    """Complete FreeCycle server configuration.

    Attributes:
        servers: List of known server entries.
        engine: Configuration for the inference engine endpoint.
        timeouts: Global timeout configuration.
        wake_on_lan: Global Wake-on-LAN configuration.
    """

    servers: list[ServerEntry]
    engine: EndpointConfig
    timeouts: TimeoutConfig
    wake_on_lan: WakeOnLanConfig


@dataclass
class LocalAvailability:
    """Status of local FreeCycle/engine availability.

    Used by ensureLocalAvailability() to report whether the local inference
    engine is available, and if not, why not (and whether Wake-on-LAN was
    attempted).

    Attributes:
        available: Whether the local engine is available and ready.
        freecycle_reachable: Whether the FreeCycle server is reachable.
        engine_reachable: Whether the inference engine is reachable.
        wake_on_lan_enabled: Whether Wake-on-LAN is enabled in config.
        wake_on_lan_attempted: Whether Wake-on-LAN packets were sent during this check.
        freecycle_status: Current status reported by FreeCycle (e.g., "Available").
        blocking_processes: List of process names blocking GPU access.
        reason: Human-readable explanation of the current state.
    """

    available: bool
    freecycle_reachable: bool = False
    engine_reachable: bool = False
    wake_on_lan_enabled: bool = False
    wake_on_lan_attempted: bool = False
    freecycle_status: Optional[str] = None
    blocking_processes: list[str] = field(default_factory=list)
    reason: str = ""


@dataclass
class ServerProbeResult:
    """Result of probing a single server during multi-server queries.

    Attributes:
        server: The server that was probed.
        status: The FreeCycle status response from the server.
        reachable: Whether the server was successfully reached.
        free_vram_mb: Amount of free VRAM on the server (in MB).
    """

    server: ServerEntry
    status: dict
    reachable: bool = True
    free_vram_mb: float = 0.0


@dataclass
class ServerProbeError:
    """Result of a failed server probe during multi-server queries.

    Attributes:
        server: The server that could not be reached.
        reachable: Always False for this class.
        error: Human-readable error message describing the failure.
    """

    server: ServerEntry
    reachable: bool = False
    error: str = ""


__all__ = [
    # Version and metadata
    "__version__",
    # Exceptions
    "FreeCycleError",
    "FreeCycleConnectionError",
    "FreeCycleTimeoutError",
    "FreeCycleUnavailableError",
    "TLSFingerprintMismatchError",
    "TaskConflictError",
    "ConfigError",
    # Dataclasses
    "EndpointConfig",
    "TimeoutConfig",
    "WakeOnLanConfig",
    "ServerEntry",
    "McpServerConfig",
    "LocalAvailability",
    "ServerProbeResult",
    "ServerProbeError",
    # Config API
    "CONFIG_PATH",
    "get_config_path",
    "get_config",
    "reset_config_cache",
    "get_active_server",
    "save_config",
    # Module functions (to be added in later tasks)
    # "FreeCycleClient",  # Will be added when the class is defined
    # ... sync wrappers, CLI helpers, etc.
]


# ============================================================================
# Configuration — Module-Level Cache and File I/O
# ============================================================================

# Module-level cache
_config_cache: Optional[McpServerConfig] = None

# Default configuration (mirrors Node.js DEFAULT_CONFIG)
DEFAULT_SERVER_DICT = {
    "host": DEFAULT_FC_HOST,
    "port": DEFAULT_FC_PORT,
    "approved": True,
}

DEFAULT_CONFIG_DICT = {
    "servers": [DEFAULT_SERVER_DICT],
    "engine": {
        "host": DEFAULT_ENGINE_HOST,
        "port": DEFAULT_ENGINE_PORT,
    },
    "timeouts": {
        "requestSecs": DEFAULT_REQUEST_SECS,
        "inferenceSecs": DEFAULT_INFERENCE_SECS,
        "pullSecs": DEFAULT_PULL_SECS,
    },
    "wakeOnLan": {
        "enabled": DEFAULT_WOL_ENABLED,
        "macAddress": DEFAULT_WOL_MAC,
        "broadcastAddress": DEFAULT_WOL_BROADCAST,
        "port": DEFAULT_WOL_PORT,
        "packetCount": DEFAULT_WOL_PACKET_COUNT,
        "packetIntervalSecs": DEFAULT_WOL_PACKET_INTERVAL_SECS,
        "pollIntervalSecs": DEFAULT_WOL_POLL_INTERVAL_SECS,
        "maxWaitSecs": DEFAULT_WOL_MAX_WAIT_SECS,
    },
}

# Config file location
CONFIG_PATH = Path(__file__).parent / "freecycle-mcp.config.json"

# Retry parameters for Windows file locking (Dropbox/antivirus)
MAX_WRITE_RETRIES = 3
WRITE_RETRY_DELAY_SECS = 0.5


def _parse_number(value: Optional[str], fallback: float) -> float:
    """Parse a numeric string, returning fallback if invalid.

    Args:
        value: String value to parse (may be None or empty).
        fallback: Value to return if parsing fails.

    Returns:
        Parsed number or fallback value.
    """
    if value is None or value.strip() == "":
        return fallback
    try:
        parsed = float(value)
        return parsed if parsed == parsed else fallback  # Checks for NaN
    except (ValueError, TypeError):
        return fallback


def _parse_boolean(value: Optional[str], fallback: bool) -> bool:
    """Parse a boolean string, returning fallback if invalid.

    Args:
        value: String value to parse (may be None or empty).
        fallback: Value to return if parsing fails or value is invalid.

    Returns:
        True if value is "true" (case-insensitive), fallback otherwise.
    """
    if value is None or value.strip() == "":
        return fallback
    return value.strip().lower() == "true"


def _read_raw_config(config_path: Path) -> dict:
    """Read the raw config JSON file.

    Returns an empty dict if the file does not exist.

    Args:
        config_path: Path to the config file.

    Returns:
        Parsed JSON config dict, or empty dict if file missing.

    Raises:
        ConfigError: If the file exists but cannot be parsed as JSON.
    """
    if not config_path.exists():
        return {}
    try:
        text = config_path.read_text(encoding="utf-8")
        return json.loads(text)
    except Exception as e:
        raise ConfigError(f"Failed to read config from {config_path}: {e}")


def _normalize_servers_array(file_config: dict) -> list[dict]:
    """Normalize the servers array in the file config.

    Handles three cases in order:
    1. If servers array exists and is non-empty: use it
    2. If old freecycle key exists: convert to single-server list
    3. Otherwise: return default server list

    Args:
        file_config: Raw JSON config dict from the file.

    Returns:
        List of server dicts (never empty).
    """
    # Case 1: servers array exists
    if file_config.get("servers") and len(file_config["servers"]) > 0:
        return file_config["servers"]

    # Case 2: old freecycle key exists (backward compat)
    if file_config.get("freecycle"):
        freecycle_dict = file_config["freecycle"]
        freecycle_host = (
            os.environ.get("FREECYCLE_HOST")
            or freecycle_dict.get("host")
            or DEFAULT_FC_HOST
        )
        freecycle_port = _parse_number(
            os.environ.get("FREECYCLE_PORT"),
            freecycle_dict.get("port", DEFAULT_FC_PORT),
        )
        return [
            {
                "host": freecycle_host,
                "port": int(freecycle_port),
                "approved": True,
            }
        ]

    # Case 3: fallback to default
    return [DEFAULT_SERVER_DICT]


def _parse_server_entry(raw: dict) -> ServerEntry:
    """Convert a raw server dict to a ServerEntry dataclass.

    Args:
        raw: Raw server dict from JSON (with camelCase keys).

    Returns:
        ServerEntry dataclass instance.

    Raises:
        ConfigError: If required fields are missing.
    """
    try:
        host = raw.get("host", DEFAULT_FC_HOST)
        port = int(raw.get("port", DEFAULT_FC_PORT))

        # Parse optional nested configs (per-server overrides)
        wake_on_lan = None
        if raw.get("wakeOnLan"):
            wol_raw = raw["wakeOnLan"]
            wake_on_lan = WakeOnLanConfig(
                enabled=_parse_boolean(
                    None if not isinstance(wol_raw.get("enabled"), bool)
                    else str(wol_raw.get("enabled")).lower(),
                    wol_raw.get("enabled", DEFAULT_WOL_ENABLED),
                ),
                mac_address=wol_raw.get("macAddress", DEFAULT_WOL_MAC),
                broadcast_address=wol_raw.get("broadcastAddress", DEFAULT_WOL_BROADCAST),
                port=int(wol_raw.get("port", DEFAULT_WOL_PORT)),
                packet_count=int(wol_raw.get("packetCount", DEFAULT_WOL_PACKET_COUNT)),
                packet_interval_secs=_parse_number(
                    None, wol_raw.get("packetIntervalSecs", DEFAULT_WOL_PACKET_INTERVAL_SECS)
                ),
                poll_interval_secs=_parse_number(
                    None, wol_raw.get("pollIntervalSecs", DEFAULT_WOL_POLL_INTERVAL_SECS)
                ),
                max_wait_secs=_parse_number(
                    None, wol_raw.get("maxWaitSecs", DEFAULT_WOL_MAX_WAIT_SECS)
                ),
            )

        timeouts = None
        if raw.get("timeouts"):
            timeouts_raw = raw["timeouts"]
            timeouts = TimeoutConfig(
                request_secs=_parse_number(
                    None, timeouts_raw.get("requestSecs", DEFAULT_REQUEST_SECS)
                ),
                inference_secs=_parse_number(
                    None, timeouts_raw.get("inferenceSecs", DEFAULT_INFERENCE_SECS)
                ),
                pull_secs=_parse_number(
                    None, timeouts_raw.get("pullSecs", DEFAULT_PULL_SECS)
                ),
            )

        return ServerEntry(
            host=host,
            port=port,
            approved=raw.get("approved", True),
            name=raw.get("name"),
            tls_fingerprint=raw.get("tls_fingerprint"),
            identity_uuid=raw.get("identity_uuid"),
            wake_on_lan=wake_on_lan,
            timeouts=timeouts,
        )
    except Exception as e:
        raise ConfigError(f"Failed to parse server entry: {e}")


def _merge_config(file_config: dict) -> McpServerConfig:
    """Merge file config with environment variable overrides.

    Matches Node.js mergeConfig() exactly in precedence and defaults.

    Args:
        file_config: Raw JSON config dict from the file.

    Returns:
        Merged McpServerConfig with all env vars applied.
    """
    servers_list = _normalize_servers_array(file_config)
    parsed_servers = [_parse_server_entry(s) for s in servers_list]

    # Engine endpoint resolution
    engine_host = (
        os.environ.get("ENGINE_HOST")
        or file_config.get("engine", {}).get("host")
        or (parsed_servers[0].host if parsed_servers else DEFAULT_FC_HOST)
    )
    engine_port = _parse_number(
        os.environ.get("ENGINE_PORT"),
        file_config.get("engine", {}).get("port", DEFAULT_ENGINE_PORT),
    )

    # Timeouts
    timeouts_dict = file_config.get("timeouts", {})
    request_secs = _parse_number(
        os.environ.get("FREECYCLE_REQUEST_TIMEOUT_SECS"),
        timeouts_dict.get("requestSecs", DEFAULT_REQUEST_SECS),
    )
    inference_secs = _parse_number(
        os.environ.get("FREECYCLE_INFERENCE_TIMEOUT_SECS"),
        timeouts_dict.get("inferenceSecs", DEFAULT_INFERENCE_SECS),
    )
    pull_secs = _parse_number(
        os.environ.get("FREECYCLE_PULL_TIMEOUT_SECS"),
        timeouts_dict.get("pullSecs", DEFAULT_PULL_SECS),
    )

    # Wake-on-LAN
    wol_dict = file_config.get("wakeOnLan", {})
    wol_enabled = _parse_boolean(
        os.environ.get("FREECYCLE_WOL_ENABLED"),
        wol_dict.get("enabled", DEFAULT_WOL_ENABLED),
    )
    wol_mac = (
        os.environ.get("FREECYCLE_WOL_MAC")
        or wol_dict.get("macAddress", DEFAULT_WOL_MAC)
    )
    wol_broadcast = (
        os.environ.get("FREECYCLE_WOL_BROADCAST")
        or wol_dict.get("broadcastAddress", DEFAULT_WOL_BROADCAST)
    )
    wol_port = _parse_number(
        os.environ.get("FREECYCLE_WOL_PORT"),
        wol_dict.get("port", DEFAULT_WOL_PORT),
    )
    wol_packet_count = _parse_number(
        os.environ.get("FREECYCLE_WOL_PACKET_COUNT"),
        wol_dict.get("packetCount", DEFAULT_WOL_PACKET_COUNT),
    )
    wol_packet_interval = _parse_number(
        os.environ.get("FREECYCLE_WOL_PACKET_INTERVAL_SECS"),
        wol_dict.get("packetIntervalSecs", DEFAULT_WOL_PACKET_INTERVAL_SECS),
    )
    wol_poll_interval = _parse_number(
        os.environ.get("FREECYCLE_WOL_POLL_INTERVAL_SECS"),
        wol_dict.get("pollIntervalSecs", DEFAULT_WOL_POLL_INTERVAL_SECS),
    )
    wol_max_wait = _parse_number(
        os.environ.get("FREECYCLE_WOL_MAX_WAIT_SECS"),
        wol_dict.get("maxWaitSecs", DEFAULT_WOL_MAX_WAIT_SECS),
    )

    return McpServerConfig(
        servers=parsed_servers,
        engine=EndpointConfig(host=engine_host, port=int(engine_port)),
        timeouts=TimeoutConfig(
            request_secs=request_secs,
            inference_secs=inference_secs,
            pull_secs=pull_secs,
        ),
        wake_on_lan=WakeOnLanConfig(
            enabled=wol_enabled,
            mac_address=wol_mac,
            broadcast_address=wol_broadcast,
            port=int(wol_port),
            packet_count=int(wol_packet_count),
            packet_interval_secs=wol_packet_interval,
            poll_interval_secs=wol_poll_interval,
            max_wait_secs=wol_max_wait,
        ),
    )


def _server_entry_to_dict(entry: ServerEntry) -> dict:
    """Convert a ServerEntry dataclass back to a raw config dict.

    Used by save_config() to serialize ServerEntry objects back to
    the JSON schema format (with camelCase keys).

    Args:
        entry: ServerEntry dataclass to serialize.

    Returns:
        Dict suitable for JSON serialization with camelCase keys.
    """
    result = {
        "host": entry.host,
        "port": entry.port,
        "approved": entry.approved,
    }

    if entry.name:
        result["name"] = entry.name

    if entry.tls_fingerprint:
        result["tls_fingerprint"] = entry.tls_fingerprint

    if entry.identity_uuid:
        result["identity_uuid"] = entry.identity_uuid

    if entry.wake_on_lan:
        result["wakeOnLan"] = {
            "enabled": entry.wake_on_lan.enabled,
            "macAddress": entry.wake_on_lan.mac_address,
            "broadcastAddress": entry.wake_on_lan.broadcast_address,
            "port": entry.wake_on_lan.port,
            "packetCount": entry.wake_on_lan.packet_count,
            "packetIntervalSecs": entry.wake_on_lan.packet_interval_secs,
            "pollIntervalSecs": entry.wake_on_lan.poll_interval_secs,
            "maxWaitSecs": entry.wake_on_lan.max_wait_secs,
        }

    if entry.timeouts:
        result["timeouts"] = {
            "requestSecs": entry.timeouts.request_secs,
            "inferenceSecs": entry.timeouts.inference_secs,
            "pullSecs": entry.timeouts.pull_secs,
        }

    return result


def _write_config_atomic(config_path: Path, data: dict) -> None:
    """Write config to disk atomically with retry on Windows.

    Writes to a temp file first, then renames it to the target path.
    On Windows, retries up to 3 times on PermissionError (Dropbox/antivirus).

    Args:
        config_path: Target config file path.
        data: Config dict to write (will be JSON-serialized).

    Raises:
        ConfigError: If the write ultimately fails after retries.
    """
    tmp_path = config_path.with_suffix(".json.tmp")

    try:
        # Write to temp file
        tmp_path.write_text(json.dumps(data, indent=2), encoding="utf-8")

        # Atomic rename with retry on Windows
        for attempt in range(MAX_WRITE_RETRIES):
            try:
                os.replace(tmp_path, config_path)
                return
            except PermissionError:
                if attempt < MAX_WRITE_RETRIES - 1:
                    time.sleep(WRITE_RETRY_DELAY_SECS)
                else:
                    raise

    except Exception as e:
        # Clean up temp file if write failed
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except Exception:
            pass  # Ignore cleanup errors
        raise ConfigError(f"Failed to write config to {config_path}: {e}")


def get_config_path() -> Path:
    """Get the config file path, with environment override.

    Returns:
        Path to the config file (from FREECYCLE_MCP_CONFIG env var
        or default freecycle-mcp.config.json in the module directory).
    """
    env_path = os.environ.get("FREECYCLE_MCP_CONFIG")
    return Path(env_path) if env_path else CONFIG_PATH


def get_config() -> McpServerConfig:
    """Get the merged configuration, loading from file if needed.

    If the config file does not exist, creates it with defaults.
    The config is cached after the first load.

    Returns:
        Merged McpServerConfig with all environment overrides applied.

    Raises:
        ConfigError: If the config file cannot be read or created.
    """
    global _config_cache

    if _config_cache is not None:
        return _config_cache

    config_path = get_config_path()

    # Create default config if file missing
    if not config_path.exists():
        try:
            config_path.parent.mkdir(parents=True, exist_ok=True)
            _write_config_atomic(config_path, DEFAULT_CONFIG_DICT)
        except Exception as e:
            raise ConfigError(
                f"Config file not found at {config_path} and could not be created: {e}"
            )

    file_config = _read_raw_config(config_path)
    _config_cache = _merge_config(file_config)
    return _config_cache


def reset_config_cache() -> None:
    """Clear the cached configuration.

    The next call to get_config() will reload from disk.
    """
    global _config_cache
    _config_cache = None


def get_active_server() -> ServerEntry:
    """Get the active server entry to use for FreeCycle operations.

    Returns the first approved server if any exist, otherwise the first
    server in the list, otherwise a default fallback server.

    Returns:
        ServerEntry to use for API calls.
    """
    config = get_config()

    # Prefer first approved server
    for server in config.servers:
        if server.approved:
            return server

    # Fallback to first server if any exist
    if config.servers:
        return config.servers[0]

    # Absolute fallback
    return ServerEntry(
        host=DEFAULT_FC_HOST,
        port=DEFAULT_FC_PORT,
        approved=True,
    )


def save_config(patch: dict) -> None:
    """Save config changes to disk.

    Reads the current config file, merges the patch dict into it (using
    raw JSON keys), and writes back atomically. The patch should use
    camelCase keys (matching the JSON schema), not snake_case Python names.

    Example:
        # Add a server with a TLS fingerprint
        save_config({
            "servers": [
                {
                    "host": "192.168.1.100",
                    "port": 7443,
                    "approved": True,
                    "tls_fingerprint": "a1b2c3...",
                }
            ]
        })

    Args:
        patch: Dict with raw config keys (camelCase) to merge into file config.

    Raises:
        ConfigError: If the config cannot be read or written.
    """
    config_path = get_config_path()
    file_config = _read_raw_config(config_path)

    # Merge patch into file config (shallow merge)
    updated = {**file_config, **patch}

    _write_config_atomic(config_path, updated)
    reset_config_cache()
