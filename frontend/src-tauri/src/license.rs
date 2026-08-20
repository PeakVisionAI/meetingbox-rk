use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager, Runtime};

const PRODUCT: &str = "meetingbox-rk";
const LICENSE_FILE_NAME: &str = "license.json";
const PUBLIC_KEY: [u8; 32] = [
    0xa1, 0x58, 0x54, 0xc9, 0x31, 0x4b, 0x8f, 0x4b, 0x8e, 0x6b, 0x3d, 0xcd, 0xc8, 0x4b, 0xea, 0xe1,
    0xf4, 0xe5, 0x22, 0xaf, 0x79, 0x50, 0x8a, 0x60, 0x29, 0x74, 0x65, 0xe2, 0x48, 0x59, 0x6f, 0xfb,
];

#[derive(Debug, Deserialize)]
struct LicenseEnvelope {
    payload: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
struct LicensePayload {
    schema: u8,
    product: String,
    device_id: String,
    issued_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

pub fn verify_startup_license<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if cfg!(debug_assertions)
        && std::env::var("MEETINGBOX_ALLOW_UNLICENSED_DEV").as_deref() == Ok("1")
    {
        log::warn!("Skipping license verification for an explicitly enabled debug build");
        return Ok(());
    }

    let license_path = license_paths(app)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "未找到许可证文件；请安装 {}",
                linux_license_path().display()
            )
        })?;
    let contents = fs::read(&license_path)
        .map_err(|error| format!("无法读取许可证 {}: {error}", license_path.display()))?;

    verify_license_bytes(&contents, &device_fingerprint(), Utc::now())
}

fn license_paths<R: Runtime>(app: &AppHandle<R>) -> Vec<PathBuf> {
    let mut paths = vec![linux_license_path()];
    if let Ok(app_data_dir) = app.path().app_data_dir() {
        paths.push(app_data_dir.join(LICENSE_FILE_NAME));
    }
    paths
}

fn linux_license_path() -> PathBuf {
    PathBuf::from("/etc/meetingbox").join(LICENSE_FILE_NAME)
}

pub fn device_fingerprint() -> String {
    let source = machine_identity_source();
    let mut digest = Sha256::new();
    digest.update(source.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn machine_identity_source() -> String {
    let mut identities = Vec::new();
    let mut add_identity = |identity: String| {
        if !identities.contains(&identity) {
            identities.push(identity);
        }
    };
    for path in [
        Path::new("/sys/firmware/devicetree/base/serial-number"),
        Path::new("/sys/class/dmi/id/product_uuid"),
        Path::new("/sys/class/dmi/id/product_serial"),
    ] {
        if let Ok(value) = fs::read_to_string(path) {
            let value = value
                .trim_matches(|character: char| character.is_whitespace() || character == '\0');
            if !value.is_empty() {
                add_identity(format!("hardware:{value}"));
            }
        }
    }
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if let Some(serial) = cpuinfo.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim().eq_ignore_ascii_case("serial")).then(|| value.trim())
        }) {
            if !serial.is_empty() {
                add_identity(format!("cpu:{serial}"));
            }
        }
    }
    for path in [
        Path::new("/etc/machine-id"),
        Path::new("/var/lib/dbus/machine-id"),
    ] {
        if let Ok(value) = fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                add_identity(format!("system:{value}"));
            }
        }
    }
    if !identities.is_empty() {
        return identities.join("|");
    }

    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown-device".to_owned())
}

fn verify_license_bytes(
    contents: &[u8],
    expected_device_id: &str,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let envelope: LicenseEnvelope =
        serde_json::from_slice(contents).map_err(|_| "许可证格式无效".to_owned())?;
    let payload_bytes = STANDARD
        .decode(envelope.payload)
        .map_err(|_| "许可证载荷无效".to_owned())?;
    let signature = STANDARD
        .decode(envelope.signature)
        .map_err(|_| "许可证签名编码无效".to_owned())?;

    UnparsedPublicKey::new(&ED25519, PUBLIC_KEY)
        .verify(&payload_bytes, &signature)
        .map_err(|_| "许可证签名不可信".to_owned())?;

    let payload: LicensePayload =
        serde_json::from_slice(&payload_bytes).map_err(|_| "许可证载荷格式无效".to_owned())?;
    if payload.schema != 1 || payload.product != PRODUCT {
        return Err("许可证不适用于当前产品".to_owned());
    }
    if payload.device_id != expected_device_id {
        return Err("许可证与当前设备不匹配".to_owned());
    }
    if payload.issued_at > now {
        return Err("许可证尚未生效".to_owned());
    }
    if payload
        .expires_at
        .is_some_and(|expires_at| expires_at < now)
    {
        return Err("许可证已过期".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "eyJzY2hlbWEiOjEsInByb2R1Y3QiOiJtZWV0aW5nYm94LXJrIiwiZGV2aWNlX2lkIjoiZGV2aWNlLWRlbW8iLCJpc3N1ZWRfYXQiOiIyMDI2LTA4LTE5VDAwOjAwOjAwWiIsImV4cGlyZXNfYXQiOiIyMDMwLTAxLTAxVDAwOjAwOjAwWiJ9";
    const SIGNATURE: &str =
        "6lsXeBv6/9YFKaFISIiK7LFCUADTxIRL4IqzGr+BrykW+vn0GvIgrJEmJEZfkEE/+DpYMmy0Wvp1zcGgCkcuCQ==";
    const SCRIPT_PAYLOAD: &str = "eyJzY2hlbWEiOjEsInByb2R1Y3QiOiJtZWV0aW5nYm94LXJrIiwiZGV2aWNlX2lkIjoic2hhMjU2OmE0YzcyODYyYzBhYmVmYjE3ZWU5ZGIyMGFlZGQ0YWVhMGViZjU4OWQ3NDk4ZTVjMWZiOTQ4MTUzMzcwNjAxZTEiLCJpc3N1ZWRfYXQiOiIyMDI2LTA4LTIwVDA1OjAwOjAwWiIsImV4cGlyZXNfYXQiOiIyMDI3LTA4LTIwVDA1OjAwOjAwWiJ9";
    const SCRIPT_SIGNATURE: &str =
        "DvFYBVryCXrDbBnfyZ+DvytUsfhmXCcdFDceJRR6OEugiPyUKQZ5WACGtX0davWuAfoxVeZE0Y2Aoe7snx3OAA==";

    fn license() -> Vec<u8> {
        format!(r#"{{"payload":"{PAYLOAD}","signature":"{SIGNATURE}"}}"#).into_bytes()
    }

    #[test]
    fn accepts_a_signed_matching_unexpired_license() {
        let now = DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(verify_license_bytes(&license(), "device-demo", now).is_ok());
    }

    #[test]
    fn accepts_license_created_by_signing_script() {
        let license = format!(
            r#"{{"payload":"{SCRIPT_PAYLOAD}","signature":"{SCRIPT_SIGNATURE}"}}"#
        );
        let now = DateTime::parse_from_rfc3339("2026-08-20T05:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(verify_license_bytes(
            license.as_bytes(),
            "sha256:a4c72862c0abefb17ee9db20aedd4aea0ebf589d7498e5c1fb948153370601e1",
            now,
        )
        .is_ok());
    }

    #[test]
    fn rejects_a_license_for_a_different_device() {
        let now = DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            verify_license_bytes(&license(), "other-device", now).unwrap_err(),
            "许可证与当前设备不匹配"
        );
    }

    #[test]
    fn rejects_an_expired_license() {
        let now = DateTime::parse_from_rfc3339("2031-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            verify_license_bytes(&license(), "device-demo", now).unwrap_err(),
            "许可证已过期"
        );
    }

    #[test]
    fn rejects_a_tampered_signature() {
        let mut bytes = license();
        bytes[20] = b'x';
        let now = DateTime::parse_from_rfc3339("2026-08-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(verify_license_bytes(&bytes, "device-demo", now).is_err());
    }
}
