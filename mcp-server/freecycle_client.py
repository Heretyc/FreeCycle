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
    # Module functions (to be added in later tasks)
    # "FreeCycleClient",  # Will be added when the class is defined
    # ... sync wrappers, CLI helpers, etc.
]
