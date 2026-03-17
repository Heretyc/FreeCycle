/**
 * Secure HTTP client for TLS connections with TOFU (Trust On First Use) fingerprint verification.
 *
 * Supports:
 * - Self-signed certificate acceptance for known FreeCycle servers
 * - SHA-256 fingerprint extraction and verification via node:tls
 * - TLS-first with plaintext fallback for compatibility mode detection
 * - Fetch-compatible wrapper (returns standard Response objects)
 */

import { request as httpsRequest } from "node:https";
import { request as httpRequest } from "node:http";
import { connect as tlsConnect } from "node:tls";
import { createHash } from "node:crypto";
import type { IncomingMessage, RequestOptions } from "node:http";
import type { ServerEntry } from "./config.js";

interface TlsSessionCache {
  [key: string]: "https" | "http";
}

const tlsSessionCache: TlsSessionCache = {};

/** Compute SHA-256 fingerprint from a DER-encoded certificate buffer. */
function computeFingerprint(derBuffer: Buffer): string {
  return createHash("sha256").update(derBuffer).digest("hex");
}

/** Convert node:http IncomingMessage to a standard Response. */
function toResponse(msg: IncomingMessage, body: Buffer): Response {
  const headers = new Headers();
  for (const [key, value] of Object.entries(msg.headers)) {
    if (value != null) {
      if (Array.isArray(value)) {
        for (const v of value) headers.append(key, v);
      } else {
        headers.set(key, value);
      }
    }
  }

  return new Response(body.toString("utf-8"), {
    status: msg.statusCode ?? 200,
    statusText: msg.statusMessage ?? "",
    headers,
  });
}

/** Execute an HTTP/HTTPS request and return a standard Response. */
function nodeRequest(
  url: string,
  init?: RequestInit,
  rejectUnauthorized?: boolean,
): Promise<Response> {
  const parsed = new URL(url);
  const isHttps = parsed.protocol === "https:";
  const requestFn = isHttps ? httpsRequest : httpRequest;

  return new Promise((resolve, reject) => {
    const options: RequestOptions & { rejectUnauthorized?: boolean } = {
      hostname: parsed.hostname,
      port: parsed.port || (isHttps ? 443 : 80),
      path: parsed.pathname + parsed.search,
      method: (init?.method ?? "GET").toUpperCase(),
    };

    // Forward request headers
    if (init?.headers) {
      if (init.headers instanceof Headers) {
        const headerObj: Record<string, string> = {};
        init.headers.forEach((value, key) => {
          headerObj[key] = value;
        });
        options.headers = headerObj;
      } else if (typeof init.headers === "object") {
        options.headers = init.headers as Record<string, string>;
      }
    }

    if (isHttps && rejectUnauthorized === false) {
      options.rejectUnauthorized = false;
    }

    const req = requestFn(options, (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (chunk: Buffer) => chunks.push(chunk));
      res.on("end", () => resolve(toResponse(res, Buffer.concat(chunks))));
      res.on("error", reject);
    });

    // Wire up AbortSignal
    if (init?.signal) {
      if (init.signal.aborted) {
        req.destroy();
        reject(new DOMException("The operation was aborted.", "AbortError"));
        return;
      }
      init.signal.addEventListener(
        "abort",
        () => {
          req.destroy();
          reject(new DOMException("The operation was aborted.", "AbortError"));
        },
        { once: true },
      );
    }

    req.on("error", reject);

    if (init?.body != null) {
      req.write(init.body);
    }
    req.end();
  });
}

/**
 * Fetch with self-signed TLS support and TOFU fingerprint verification.
 *
 * When a ServerEntry is provided:
 * - Connects with rejectUnauthorized: false (accepts self-signed certs)
 * - If entry.tls_fingerprint is set: verifies the server cert matches
 *
 * When no entry is provided:
 * - Falls back to TLS-first with plaintext fallback (for unknown servers)
 * - Uses normal certificate validation for the initial probe
 */
export async function secureFetch(
  url: string,
  entry?: ServerEntry,
  init?: RequestInit,
): Promise<Response> {
  if (!url.startsWith("https://") && !url.startsWith("http://")) {
    url = `https://${url}`;
  }

  // Known server: use self-signed TLS acceptance with optional TOFU verification
  if (entry) {
    const parsed = new URL(url);
    const isHttps = parsed.protocol === "https:";

    if (isHttps) {
      // Verify fingerprint if one is pinned
      if (
        entry.tls_fingerprint &&
        entry.tls_fingerprint !== "pending-verification"
      ) {
        const actualFingerprint = await extractServerFingerprint(
          parsed.hostname,
          Number(parsed.port) || 443,
        );
        if (!verifyFingerprint(entry.tls_fingerprint, actualFingerprint)) {
          throw new Error(
            `TLS fingerprint mismatch for ${parsed.hostname}:${parsed.port}. ` +
              `Expected: ${entry.tls_fingerprint.slice(0, 16)}..., ` +
              `got: ${actualFingerprint.slice(0, 16)}... ` +
              `This could indicate a man-in-the-middle attack or server certificate rotation.`,
          );
        }
      }

      // Accept self-signed cert for this known server
      return nodeRequest(url, init, false);
    }

    // HTTP URL for a known server: just request directly
    return nodeRequest(url, init);
  }

  // No server entry: use cached protocol detection with fallback
  const cacheKey = new URL(url).hostname;
  const cached = tlsSessionCache[cacheKey];

  if (cached === "http") {
    const httpUrl = url.replace(/^https:\/\//, "http://");
    try {
      return await nodeRequest(httpUrl, init);
    } catch {
      delete tlsSessionCache[cacheKey];
    }
  }

  if (cached === "https") {
    try {
      return await nodeRequest(url, init);
    } catch {
      delete tlsSessionCache[cacheKey];
    }
  }

  // Unknown mode (or cache invalidated): try HTTPS first, fall back to plaintext
  let httpsError: unknown;
  try {
    const response = await nodeRequest(url, init);
    tlsSessionCache[cacheKey] = "https";
    return response;
  } catch (err) {
    httpsError = err;
  }

  let httpError: unknown;
  try {
    const httpUrl = url.replace(/^https:\/\//, "http://");
    const response = await nodeRequest(httpUrl, init);
    tlsSessionCache[cacheKey] = "http";
    return response;
  } catch (err) {
    httpError = err;
  }

  // Both failed: include both errors for debuggability
  const httpsMsg =
    httpsError instanceof Error ? httpsError.message : String(httpsError);
  const httpMsg =
    httpError instanceof Error ? httpError.message : String(httpError);
  throw new Error(
    `Connection to ${cacheKey} failed. HTTPS: ${httpsMsg}. HTTP fallback: ${httpMsg}`,
  );
}

/**
 * Extract the SHA-256 fingerprint of a server's TLS certificate.
 *
 * Performs a TLS handshake with rejectUnauthorized: false to accept
 * self-signed certificates, then computes the fingerprint from the
 * DER-encoded certificate.
 */
export async function extractServerFingerprint(
  host: string,
  port: number,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const socket = tlsConnect(
      { host, port, rejectUnauthorized: false },
      () => {
        const cert = socket.getPeerCertificate(false);
        if (!cert || !cert.raw) {
          socket.destroy();
          reject(
            new Error(
              `Failed to extract TLS certificate from ${host}:${port}`,
            ),
          );
          return;
        }

        const fingerprint = computeFingerprint(cert.raw);
        socket.destroy();
        resolve(fingerprint);
      },
    );

    socket.on("error", (err: Error) => {
      socket.destroy();
      reject(
        new Error(
          `TLS connection to ${host}:${port} failed: ${err.message}`,
        ),
      );
    });

    socket.setTimeout(5000, () => {
      socket.destroy();
      reject(
        new Error(`TLS handshake with ${host}:${port} timed out after 5s`),
      );
    });
  });
}

export function verifyFingerprint(expected: string, actual: string): boolean {
  return expected.toLowerCase() === actual.toLowerCase();
}
