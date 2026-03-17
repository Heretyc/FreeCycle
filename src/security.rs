//! Security-related cryptographic operations for FreeCycle.
//!
//! Handles Ed25519 keypair generation, storage, and retrieval for secure mode operations.
//! This module is the home for all Priority 7+ cryptographic operations including TLS certificates,
//! GPU fingerprints, and identity management.

use anyhow::{Context, Result};
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey};
use ed25519_dalek::SigningKey;
use pkcs8::LineEnding;
use rand_core::OsRng;
use std::path::{Path, PathBuf};
use tracing::info;

use crate::config::{config_dir, FreeCycleConfig, SecurityConfig};
use base64::Engine;
use ed25519_dalek::pkcs8::DecodePublicKey;
use sha2::Digest;
use sha2::Sha256;

const SIGNING_KEY_FILENAME: &str = "freecycle_signing_key.pem";
const VERIFYING_KEY_FILENAME: &str = "freecycle_verifying_key.pem";
const TLS_CERT_FILENAME: &str = "freecycle_cert.pem";
const TLS_KEY_FILENAME: &str = "freecycle_key.pem";

/// Resolves the keypair directory path from the security config.
///
/// If `config.keypair_path` is set, uses it as-is.
/// Otherwise, defaults to the configuration directory (`%APPDATA%\FreeCycle\`).
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// The resolved directory path.
pub fn resolve_keypair_dir(config: &SecurityConfig) -> PathBuf {
    config
        .keypair_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(config_dir)
}

/// Resolves the TLS certificate directory path from the security config.
///
/// If `config.cert_path` is set, uses it as-is.
/// Otherwise, defaults to the configuration directory (`%APPDATA%\FreeCycle\`).
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// The resolved directory path.
pub fn resolve_cert_dir(config: &SecurityConfig) -> PathBuf {
    config
        .cert_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(config_dir)
}

/// Returns the path to the Ed25519 signing (private) key PEM file.
fn signing_key_path(keypair_dir: &Path) -> PathBuf {
    keypair_dir.join(SIGNING_KEY_FILENAME)
}

/// Returns the path to the Ed25519 verifying (public) key PEM file.
fn verifying_key_path(keypair_dir: &Path) -> PathBuf {
    keypair_dir.join(VERIFYING_KEY_FILENAME)
}

/// Returns the path to the TLS certificate PEM file.
fn tls_cert_path(cert_dir: &Path) -> PathBuf {
    cert_dir.join(TLS_CERT_FILENAME)
}

/// Returns the path to the TLS private key PEM file.
fn tls_key_path(cert_dir: &Path) -> PathBuf {
    cert_dir.join(TLS_KEY_FILENAME)
}

/// Returns the paths to the TLS certificate and key files for the given security config.
///
/// This is a public convenience function that resolves the certificate directory,
/// computes the paths to both files, and returns them as a tuple. It is used by
/// agent_server.rs to obtain the TLS files when running in secure mode.
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// A tuple of (cert_path, key_path).
pub fn tls_cert_and_key_paths(config: &SecurityConfig) -> (PathBuf, PathBuf) {
    let cert_dir = resolve_cert_dir(config);
    let cert_path = tls_cert_path(&cert_dir);
    let key_path = tls_key_path(&cert_dir);
    (cert_path, key_path)
}

/// Generates a new Ed25519 keypair and writes both PEM files.
///
/// # Arguments
///
/// * `keypair_dir` - The directory to write the keypair files to
///
/// # Returns
///
/// The generated `SigningKey`.
fn generate_new_keypair(keypair_dir: &Path) -> Result<SigningKey> {
    // Ensure the keypair directory exists
    std::fs::create_dir_all(keypair_dir)
        .context("Failed to create keypair directory")?;

    // Generate a new signing key from random bytes
    let mut secret_bytes = [0u8; 32];
    use rand_core::RngCore;
    OsRng.fill_bytes(&mut secret_bytes);
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let verifying_key = signing_key.verifying_key();

    // Write the private key to PKCS#8 PEM
    let signing_pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .context("Failed to encode signing key to PEM")?;
    let signing_path = signing_key_path(keypair_dir);
    std::fs::write(&signing_path, signing_pem.as_bytes())
        .context("Failed to write signing key PEM file")?;

    // Write the public key to SubjectPublicKeyInfo PEM
    let verifying_pem = verifying_key
        .to_public_key_pem(LineEnding::LF)
        .context("Failed to encode verifying key to PEM")?;
    let verifying_path = verifying_key_path(keypair_dir);
    std::fs::write(&verifying_path, verifying_pem.as_bytes())
        .context("Failed to write verifying key PEM file")?;

    info!(
        "Generated new Ed25519 keypair at {}",
        signing_path.display()
    );

    Ok(signing_key)
}

/// Loads an existing Ed25519 keypair from PEM files.
///
/// # Arguments
///
/// * `keypair_dir` - The directory containing the keypair files
///
/// # Returns
///
/// The loaded `SigningKey`, or an error if either file does not exist or cannot be parsed.
fn load_existing_keypair(keypair_dir: &Path) -> Result<SigningKey> {
    let signing_path = signing_key_path(keypair_dir);

    let pem_data = std::fs::read_to_string(&signing_path)
        .context("Failed to read signing key PEM file")?;

    let signing_key = SigningKey::from_pkcs8_pem(pem_data.as_str())
        .context("Failed to parse signing key from PEM")?;

    Ok(signing_key)
}

/// Ensures the Ed25519 keypair exists, generating it if necessary.
///
/// - If both PEM files exist: loads and returns the signing key (idempotent).
/// - If neither file exists: generates a new keypair, writes both files, and returns the signing key.
/// - If one file exists but the other does not: logs a warning, regenerates both files, and returns the signing key.
///
/// The public key is always derivable from the signing key via `signing_key.verifying_key()`.
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// The Ed25519 signing key, or an error if generation/loading fails.
pub fn ensure_keypair(config: &SecurityConfig) -> Result<SigningKey> {
    let keypair_dir = resolve_keypair_dir(config);
    let signing_path = signing_key_path(&keypair_dir);
    let verifying_path = verifying_key_path(&keypair_dir);

    let signing_exists = signing_path.exists();
    let verifying_exists = verifying_path.exists();

    match (signing_exists, verifying_exists) {
        // Both exist: load and return
        (true, true) => load_existing_keypair(&keypair_dir),

        // Neither exists: generate both
        (false, false) => generate_new_keypair(&keypair_dir),

        // Partial state: regenerate both with warning
        _ => {
            tracing::warn!(
                "Keypair files in partial state at {}; regenerating",
                keypair_dir.display()
            );
            generate_new_keypair(&keypair_dir)
        }
    }
}

/// Generates a new self-signed TLS certificate and writes both PEM files.
///
/// Creates an ECDSA P-256 self-signed certificate with:
/// - DNS SAN: localhost
/// - IP SANs: 127.0.0.1, 0.0.0.0
/// - Validity: 30 years from now
///
/// # Arguments
///
/// * `cert_dir` - The directory to write the certificate files to
///
/// # Returns
///
/// `Result<()>` on success, error if generation or writing fails.
fn generate_new_tls_cert(cert_dir: &Path) -> Result<()> {
    use pem::Pem;

    // Ensure the certificate directory exists
    std::fs::create_dir_all(cert_dir)
        .context("Failed to create certificate directory")?;

    // Use generate_simple_self_signed for a basic self-signed cert
    // with DNS and IP SANs
    let subject_alt_names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "0.0.0.0".to_string(),
    ];

    let cert_key = rcgen::generate_simple_self_signed(subject_alt_names)
        .context("Failed to generate self-signed certificate")?;

    // Serialize certificate DER to PEM
    // rcgen::Certificate.der() returns CertificateDer; convert to Vec<u8>
    let cert_der_bytes = cert_key.cert.der().as_ref().to_vec();
    let cert_pem = Pem::new("CERTIFICATE".to_string(), cert_der_bytes);
    let cert_pem_string = pem::encode(&cert_pem);

    // Get key PEM from the key pair
    let key_pem = cert_key.key_pair.serialize_pem();

    // Write the certificate PEM
    let cert_path = tls_cert_path(cert_dir);
    std::fs::write(&cert_path, cert_pem_string)
        .context("Failed to write certificate PEM file")?;

    // Write the private key PEM
    let key_path = tls_key_path(cert_dir);
    std::fs::write(&key_path, key_pem)
        .context("Failed to write private key PEM file")?;

    info!(
        "Generated new self-signed TLS certificate at {}",
        cert_path.display()
    );

    Ok(())
}

/// Ensures the TLS certificate and key files exist, generating them if necessary.
///
/// - If both files exist: returns success with `false` (idempotent, no regeneration).
/// - If neither file exists: generates a new certificate, writes both files, returns `true`.
/// - If one file exists but the other does not: logs a warning, regenerates both files, returns `true`.
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// `Result<bool>` where `true` means the cert was newly generated or regenerated (partial-state recovery),
/// and `false` means both files already existed. An error is returned if generation/writing fails.
pub fn ensure_tls_cert(config: &SecurityConfig) -> Result<bool> {
    let cert_dir = resolve_cert_dir(config);
    let cert_path = tls_cert_path(&cert_dir);
    let key_path = tls_key_path(&cert_dir);

    let cert_exists = cert_path.exists();
    let key_exists = key_path.exists();

    match (cert_exists, key_exists) {
        // Both exist: success (idempotent), return false
        (true, true) => Ok(false),

        // Neither exists: generate both, return true
        (false, false) => generate_new_tls_cert(&cert_dir).map(|()| true),

        // Partial state: regenerate both with warning, return true
        _ => {
            tracing::warn!(
                "TLS certificate files in partial state at {}; regenerating",
                cert_dir.display()
            );
            generate_new_tls_cert(&cert_dir).map(|()| true)
        }
    }
}

/// Ensures the server UUID exists, generating it if necessary.
///
/// - If `config.security.identity_uuid` is `Some(ref s)`: returns `s.clone()` (idempotent, no disk write).
/// - If `None`: generates UUID v4 via `uuid::Uuid::new_v4()`, formats as hyphenated string,
///   sets `config.security.identity_uuid = Some(uuid_str.clone())`, calls `config.save()`,
///   and returns `uuid_str`.
///
/// # Arguments
///
/// * `config` - The mutable FreeCycle configuration
///
/// # Returns
///
/// The server UUID as a hyphenated string, or an error if generation/saving fails.
pub fn ensure_identity_uuid(config: &mut FreeCycleConfig) -> Result<String> {
    // If UUID already exists, return it without modifying config or writing to disk
    if let Some(ref uuid_str) = config.security.identity_uuid {
        return Ok(uuid_str.clone());
    }

    // Generate a new UUID v4
    let new_uuid = uuid::Uuid::new_v4();
    let uuid_str = new_uuid.to_string(); // Hyphenated format

    // Update the config in memory
    config.security.identity_uuid = Some(uuid_str.clone());

    // Persist to disk
    config.save().context("Failed to save config with identity_uuid")?;

    info!("Generated new server UUID: {}", uuid_str);

    Ok(uuid_str)
}

/// Formats GPU fingerprint into the standard string: "{local_ip} with {gpu_name} @ {vram_total_mb}MB VRAM"
///
/// This is a pure function for testing; the public `build_gpu_fingerprint` wraps it with NVML integration.
///
/// # Arguments
///
/// * `local_ip` - The local IP address (as a string)
/// * `gpu_name` - The GPU model name (as a string)
/// * `vram_total_mb` - Total VRAM in megabytes
///
/// # Returns
///
/// A formatted fingerprint string.
fn format_gpu_fingerprint(local_ip: &str, gpu_name: &str, vram_total_mb: u64) -> String {
    format!("{} with {} @ {}MB VRAM", local_ip, gpu_name, vram_total_mb)
}

/// Builds GPU fingerprint: "{local_ip} with {gpu_name} @ {vram_total_mb}MB VRAM"
///
/// This function queries the local IP address and GPU information via NVML, then formats them
/// into a fingerprint string. It gracefully falls back to placeholder strings when hardware
/// information is unavailable.
///
/// If `config.fingerprint_override` is set, returns the override value immediately without
/// querying NVML. This allows users with non-standard setups (multi-GPU, VM, headless) to
/// provide an explicit fingerprint in config.toml.
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// A fingerprint string (never fails, always returns a non-empty value).
///
/// # Fallbacks
///
/// - **Local IP**: Falls back to `"unknown"` if `local_ip_address::local_ip()` fails.
/// - **GPU name**: Falls back to `"unknown GPU"` if NVML initialization or `device.name()` fails.
/// - **VRAM**: Falls back to `0` if VRAM query fails.
pub fn build_gpu_fingerprint(config: &SecurityConfig) -> String {
    // If override is set, return it immediately
    if let Some(ref override_fp) = config.fingerprint_override {
        return override_fp.clone();
    }

    // Resolve local IP
    let local_ip = match local_ip_address::local_ip() {
        Ok(ip) => ip.to_string(),
        Err(_) => "unknown".to_string(),
    };

    // Initialize NVML and query GPU 0
    let gpu_name = match nvml_wrapper::Nvml::init() {
        Ok(nvml) => {
            match nvml.device_by_index(0) {
                Ok(device) => {
                    match device.name() {
                        Ok(name) => name,
                        Err(_) => "unknown GPU".to_string(),
                    }
                }
                Err(_) => "unknown GPU".to_string(),
            }
        }
        Err(_) => "unknown GPU".to_string(),
    };

    // Initialize NVML again to query VRAM (independent from above)
    // TODO(multi-gpu): only device 0 is queried; multi-GPU support is future work
    let vram_total_mb = match nvml_wrapper::Nvml::init() {
        Ok(nvml) => {
            match nvml.device_by_index(0) {
                Ok(device) => {
                    match device.memory_info() {
                        Ok(mem_info) => mem_info.total / (1024 * 1024),
                        Err(_) => 0,
                    }
                }
                Err(_) => 0,
            }
        }
        Err(_) => 0,
    };

    format_gpu_fingerprint(&local_ip, &gpu_name, vram_total_mb)
}

/// Reads the Ed25519 verifying (public) key from PEM file and returns it base64-encoded.
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// The base64-encoded 32-byte public key, or `None` if the file does not exist or cannot be parsed.
pub fn read_verifying_key_base64(config: &SecurityConfig) -> Option<String> {
    let keypair_dir = resolve_keypair_dir(config);
    let verifying_path = verifying_key_path(&keypair_dir);

    // Read the PEM file
    let pem_data = std::fs::read_to_string(&verifying_path).ok()?;

    // Parse the PEM string into a VerifyingKey
    let verifying_key = ed25519_dalek::VerifyingKey::from_public_key_pem(&pem_data).ok()?;

    // Extract the 32 raw bytes and base64-encode them
    let raw_bytes = verifying_key.as_bytes();
    Some(base64::engine::general_purpose::STANDARD.encode(raw_bytes))
}

/// Reads the TLS certificate from PEM file and returns its SHA-256 fingerprint as lowercase hex.
///
/// # Arguments
///
/// * `config` - The security configuration
///
/// # Returns
///
/// The SHA-256 fingerprint as a 64-character lowercase hex string, or `None` if the file does not exist or cannot be parsed.
pub fn read_tls_cert_fingerprint(config: &SecurityConfig) -> Option<String> {
    let cert_dir = resolve_cert_dir(config);
    let cert_path = tls_cert_path(&cert_dir);

    // Read the PEM file
    let pem_data = std::fs::read_to_string(&cert_path).ok()?;

    // Parse the PEM to extract DER bytes
    let pem = pem::parse(&pem_data).ok()?;
    let der_bytes = pem.contents();

    // Compute SHA-256 hash of the DER bytes
    let digest = Sha256::digest(der_bytes);

    // Format as lowercase hex string
    Some(format!("{:x}", digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, Verifier};
    use std::path::Path;
    use tempfile::TempDir;

    /// Helper to create a temporary keypair directory for testing.
    fn with_temp_dir<F>(f: F) -> Result<()>
    where
        F: Fn(&Path) -> Result<()>,
    {
        let temp_dir = TempDir::new()?;
        f(temp_dir.path())
    }

    #[test]
    fn test_keypair_gen_produces_valid_keys() {
        with_temp_dir(|temp_dir| {
            let signing_key = generate_new_keypair(temp_dir)?;
            let verifying_key = signing_key.verifying_key();

            // Test that we can sign and verify a message
            let message = b"test message";
            let signature = signing_key.sign(message);
            assert!(verifying_key.verify(message, &signature).is_ok());

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_signing_key_pem_round_trip() {
        with_temp_dir(|temp_dir| {
            let signing_key_orig = generate_new_keypair(temp_dir)?;
            let signing_key_loaded = load_existing_keypair(temp_dir)?;

            // Verify both keys sign and verify identically
            let message = b"round trip test";
            let sig1 = signing_key_orig.sign(message);
            let sig2 = signing_key_loaded.sign(message);

            let verify_key = signing_key_orig.verifying_key();
            assert!(verify_key.verify(message, &sig1).is_ok());
            assert!(verify_key.verify(message, &sig2).is_ok());

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_verifying_key_pem_round_trip() {
        use ed25519_dalek::pkcs8::DecodePublicKey;

        with_temp_dir(|temp_dir| {
            let signing_key = generate_new_keypair(temp_dir)?;
            let verifying_key_orig = signing_key.verifying_key();

            // Load the PEM file directly and parse it
            let verifying_path = verifying_key_path(temp_dir);
            let pem_data = std::fs::read_to_string(&verifying_path)?;
            let verifying_key_loaded =
                ed25519_dalek::VerifyingKey::from_public_key_pem(&pem_data)?;

            // Verify both keys produce the same verification results
            let message = b"verify round trip";
            let signature = signing_key.sign(message);
            assert!(verifying_key_orig.verify(message, &signature).is_ok());
            assert!(verifying_key_loaded.verify(message, &signature).is_ok());

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_keypair_paths_default_to_config_dir() {
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: None,
        };

        let resolved = resolve_keypair_dir(&config);
        let expected = config_dir();

        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_keypair_paths_override() {
        let override_path = "C:\\custom\\path";
        let config = SecurityConfig {
            keypair_path: Some(override_path.to_string()),
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: None,
        };

        let resolved = resolve_keypair_dir(&config);
        assert_eq!(resolved, PathBuf::from(override_path));
    }

    #[test]
    fn test_ensure_keypair_generates_on_first_run() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: Some(temp_dir.to_string_lossy().to_string()),
                cert_path: None,
                identity_uuid: None,
                fingerprint_override: None,
            };

            // First run: should generate
            let key = ensure_keypair(&config)?;

            // Verify both files were created
            assert!(signing_key_path(temp_dir).exists());
            assert!(verifying_key_path(temp_dir).exists());

            // Verify the key is valid
            let message = b"first run test";
            let signature = key.sign(message);
            let verifying_key = key.verifying_key();
            assert!(verifying_key.verify(message, &signature).is_ok());

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_ensure_keypair_loads_existing() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: Some(temp_dir.to_string_lossy().to_string()),
                cert_path: None,
                identity_uuid: None,
                fingerprint_override: None,
            };

            // First run: generate
            let key1 = ensure_keypair(&config)?;
            let verifying_key1 = key1.verifying_key();

            // Second run: should load without regenerating
            let key2 = ensure_keypair(&config)?;
            let verifying_key2 = key2.verifying_key();

            // Both keys should be identical
            assert_eq!(
                verifying_key1.to_bytes(),
                verifying_key2.to_bytes()
            );

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_ensure_keypair_regenerates_on_partial_state() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: Some(temp_dir.to_string_lossy().to_string()),
                cert_path: None,
                identity_uuid: None,
                fingerprint_override: None,
            };

            // First run: generate both files
            let _ = ensure_keypair(&config)?;

            // Delete the verifying key file to create partial state
            let verifying_path = verifying_key_path(temp_dir);
            std::fs::remove_file(&verifying_path)?;

            // Verify partial state
            assert!(signing_key_path(temp_dir).exists());
            assert!(!verifying_path.exists());

            // Second run: should regenerate both
            let key = ensure_keypair(&config)?;

            // Verify both files exist again
            assert!(signing_key_path(temp_dir).exists());
            assert!(verifying_path.exists());

            // Verify the key is valid
            let message = b"regenerated test";
            let signature = key.sign(message);
            let verifying_key = key.verifying_key();
            assert!(verifying_key.verify(message, &signature).is_ok());

            Ok(())
        })
        .expect("test should succeed");
    }

    // TLS Certificate Tests

    #[test]
    fn test_tls_cert_gen_produces_valid_pem() {
        with_temp_dir(|temp_dir| {
            generate_new_tls_cert(temp_dir)?;

            let cert_path = tls_cert_path(temp_dir);
            let key_path = tls_key_path(temp_dir);

            assert!(cert_path.exists(), "Certificate file should exist");
            assert!(key_path.exists(), "Key file should exist");

            // Check PEM headers
            let cert_content = std::fs::read_to_string(&cert_path)?;
            let key_content = std::fs::read_to_string(&key_path)?;

            assert!(
                cert_content.contains("-----BEGIN CERTIFICATE-----"),
                "Cert should contain PEM header"
            );
            assert!(
                key_content.contains("-----BEGIN PRIVATE KEY-----"),
                "Key should contain PEM header"
            );

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_ensure_tls_cert_idempotent() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: None,
                cert_path: Some(temp_dir.to_string_lossy().to_string()),
                identity_uuid: None,
                fingerprint_override: None,
            };

            // First run: should generate and return true
            let regenerated1 = ensure_tls_cert(&config)?;
            assert!(regenerated1, "First run should generate certificate");
            let cert_path = tls_cert_path(temp_dir);
            let key_path = tls_key_path(temp_dir);

            assert!(cert_path.exists());
            assert!(key_path.exists());

            // Get file modification times
            let cert_mtime1 = std::fs::metadata(&cert_path)?.modified()?;
            let key_mtime1 = std::fs::metadata(&key_path)?.modified()?;

            // Small delay to ensure any regeneration would have a different timestamp
            std::thread::sleep(std::time::Duration::from_millis(10));

            // Second run: should not regenerate and return false
            let regenerated2 = ensure_tls_cert(&config)?;
            assert!(!regenerated2, "Second run should not regenerate certificate");

            let cert_mtime2 = std::fs::metadata(&cert_path)?.modified()?;
            let key_mtime2 = std::fs::metadata(&key_path)?.modified()?;

            assert_eq!(
                cert_mtime1, cert_mtime2,
                "Certificate file should not be regenerated"
            );
            assert_eq!(
                key_mtime1, key_mtime2,
                "Key file should not be regenerated"
            );

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_ensure_tls_cert_regenerates_on_partial_state() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: None,
                cert_path: Some(temp_dir.to_string_lossy().to_string()),
                identity_uuid: None,
                fingerprint_override: None,
            };

            // First run: generate both files
            let regenerated1 = ensure_tls_cert(&config)?;
            assert!(regenerated1, "First run should generate certificate");

            let cert_path = tls_cert_path(temp_dir);
            let key_path = tls_key_path(temp_dir);

            assert!(cert_path.exists());
            assert!(key_path.exists());

            // Delete the key file to create partial state
            std::fs::remove_file(&key_path)?;

            // Verify partial state
            assert!(cert_path.exists());
            assert!(!key_path.exists());

            // Second run: should regenerate both and return true
            let regenerated2 = ensure_tls_cert(&config)?;
            assert!(regenerated2, "Partial state should trigger regeneration");

            // Verify both files exist again
            assert!(cert_path.exists());
            assert!(key_path.exists());

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_tls_cert_path_defaults_to_config_dir() {
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: None,
        };

        let resolved = resolve_cert_dir(&config);
        let expected = config_dir();

        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_tls_cert_path_override() {
        let override_path = "C:\\custom\\cert\\path";
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: Some(override_path.to_string()),
            identity_uuid: None,
            fingerprint_override: None,
        };

        let resolved = resolve_cert_dir(&config);
        assert_eq!(resolved, PathBuf::from(override_path));
    }

    // Identity UUID Tests

    #[test]
    fn test_uuid_generated_on_first_run() {
        use crate::config::FreeCycleConfig;

        let mut config = FreeCycleConfig::default();
        assert!(config.security.identity_uuid.is_none());

        // Generate UUID for the first time
        let uuid = ensure_identity_uuid(&mut config).expect("should generate UUID");

        // Verify the UUID is valid (can be parsed back)
        let parsed = uuid::Uuid::parse_str(&uuid).expect("should parse as valid UUID");
        assert_eq!(parsed.to_string(), uuid);

        // Verify the config field was updated
        assert_eq!(config.security.identity_uuid, Some(uuid));
    }

    #[test]
    fn test_uuid_not_regenerated_on_second_call() {
        use crate::config::FreeCycleConfig;

        let mut config = FreeCycleConfig::default();

        // First call: generate
        let uuid1 = ensure_identity_uuid(&mut config).expect("first call should succeed");

        // Second call: should return the same UUID
        let uuid2 = ensure_identity_uuid(&mut config).expect("second call should succeed");

        // Both should be identical
        assert_eq!(uuid1, uuid2);
    }

    #[test]
    fn test_uuid_idempotent() {
        use crate::config::FreeCycleConfig;

        let mut config = FreeCycleConfig::default();

        // Call multiple times
        let uuid1 = ensure_identity_uuid(&mut config).expect("call 1");
        let uuid2 = ensure_identity_uuid(&mut config).expect("call 2");
        let uuid3 = ensure_identity_uuid(&mut config).expect("call 3");

        // All should be identical
        assert_eq!(uuid1, uuid2);
        assert_eq!(uuid2, uuid3);
    }

    // GPU Fingerprint Tests

    #[test]
    fn test_fingerprint_format_structure() {
        let local_ip = "192.168.1.1";
        let gpu_name = "RTX 3090";
        let vram_total_mb = 24576;

        let fingerprint = format_gpu_fingerprint(local_ip, gpu_name, vram_total_mb);

        assert_eq!(
            fingerprint,
            "192.168.1.1 with RTX 3090 @ 24576MB VRAM"
        );
    }

    #[test]
    fn test_fingerprint_format_zero_vram() {
        let local_ip = "10.0.0.1";
        let gpu_name = "unknown GPU";
        let vram_total_mb = 0;

        let fingerprint = format_gpu_fingerprint(local_ip, gpu_name, vram_total_mb);

        assert_eq!(
            fingerprint,
            "10.0.0.1 with unknown GPU @ 0MB VRAM"
        );
    }

    #[test]
    fn test_fingerprint_override_is_returned_verbatim() {
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: Some("custom fingerprint".to_string()),
        };

        let fingerprint = build_gpu_fingerprint(&config);

        assert_eq!(fingerprint, "custom fingerprint");
    }

    #[test]
    fn test_fingerprint_override_none_returns_string() {
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: None,
        };

        let fingerprint = build_gpu_fingerprint(&config);

        // Just verify non-empty string; cannot assert exact format without hardware
        assert!(!fingerprint.is_empty());
        // Should contain the pattern "with" and "@ ... MB VRAM" even on fallback
        assert!(fingerprint.contains(" with "));
        assert!(fingerprint.contains("MB VRAM"));
    }

    // TLS cert and key paths helper tests

    #[test]
    fn test_tls_cert_and_key_paths_defaults_to_config_dir() {
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: None,
        };

        let (cert_path, key_path) = tls_cert_and_key_paths(&config);
        let config_dir = crate::config::config_dir();

        assert_eq!(cert_path, config_dir.join(TLS_CERT_FILENAME));
        assert_eq!(key_path, config_dir.join(TLS_KEY_FILENAME));
    }

    #[test]
    fn test_tls_cert_and_key_paths_with_override() {
        let override_path = "C:\\custom\\cert\\path";
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: Some(override_path.to_string()),
            identity_uuid: None,
            fingerprint_override: None,
        };

        let (cert_path, key_path) = tls_cert_and_key_paths(&config);

        assert_eq!(cert_path, PathBuf::from(override_path).join(TLS_CERT_FILENAME));
        assert_eq!(key_path, PathBuf::from(override_path).join(TLS_KEY_FILENAME));
    }

    // Public key and TLS fingerprint reading tests

    #[test]
    fn test_read_verifying_key_base64_returns_32_byte_base64() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: Some(temp_dir.to_string_lossy().to_string()),
                cert_path: None,
                identity_uuid: None,
                fingerprint_override: None,
            };

            // Generate a keypair first
            let _signing_key = ensure_keypair(&config)?;

            // Read the base64-encoded public key
            let base64_pubkey = read_verifying_key_base64(&config);

            // Verify it returned Some value
            assert!(base64_pubkey.is_some(), "Should return Some(base64_string)");

            let base64_str = base64_pubkey.unwrap();

            // Decode the base64 and verify it's 32 bytes
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&base64_str)
                .expect("Should decode valid base64");
            assert_eq!(decoded.len(), 32, "Public key should be 32 bytes");

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_read_verifying_key_base64_returns_none_when_file_missing() {
        let config = SecurityConfig {
            keypair_path: Some("/nonexistent/keypair/path".to_string()),
            cert_path: None,
            identity_uuid: None,
            fingerprint_override: None,
        };

        let result = read_verifying_key_base64(&config);
        assert!(result.is_none(), "Should return None when keypair files don't exist");
    }

    #[test]
    fn test_read_tls_cert_fingerprint_returns_64_hex_chars() {
        with_temp_dir(|temp_dir| {
            let config = SecurityConfig {
                keypair_path: None,
                cert_path: Some(temp_dir.to_string_lossy().to_string()),
                identity_uuid: None,
                fingerprint_override: None,
            };

            // Generate a TLS certificate first
            ensure_tls_cert(&config)?;

            // Read the fingerprint
            let fingerprint = read_tls_cert_fingerprint(&config);

            // Verify it returned Some value
            assert!(fingerprint.is_some(), "Should return Some(hex_string)");

            let fingerprint_str = fingerprint.unwrap();

            // Verify it's a 64-character lowercase hex string
            assert_eq!(
                fingerprint_str.len(),
                64,
                "SHA-256 hex digest should be 64 chars"
            );
            assert!(
                fingerprint_str.chars().all(|c| c.is_ascii_hexdigit()),
                "Should contain only hex digits"
            );
            assert!(
                fingerprint_str.chars().all(|c| !c.is_ascii_uppercase()),
                "Should be lowercase hex"
            );

            Ok(())
        })
        .expect("test should succeed");
    }

    #[test]
    fn test_read_tls_cert_fingerprint_returns_none_when_file_missing() {
        let config = SecurityConfig {
            keypair_path: None,
            cert_path: Some("/nonexistent/cert/path".to_string()),
            identity_uuid: None,
            fingerprint_override: None,
        };

        let result = read_tls_cert_fingerprint(&config);
        assert!(result.is_none(), "Should return None when cert file doesn't exist");
    }
}
