//! `ca_file`(信頼する CA 証明書の PEM)を、`oracledb` に渡す。
//!
//! `oracledb` は TLS で、組み込みの CA(webpki)に加え、ウォレット(`ewallet.pem`)の証明書を
//! 信頼する。秘密鍵の無いウォレットは、証明書がそのまま信頼する CA に加わる。
//! そのため、`ca_file` を、`ewallet.pem` という名前で置いたディレクトリを、ウォレットとして渡す。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use driver_api::{DriverError, ErrorKind};

/// `ca_file` の内容ごとに決まるディレクトリ(同じ内容なら使い回す)を作り、そのパスを返す。
pub(crate) fn wallet_dir(ca_file: &Path) -> Result<String, DriverError> {
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        DriverError::new(
            ErrorKind::Connect,
            format!("CA 証明書のファイルを{what}: {} ({e})", ca_file.display()),
        )
    };
    let pem = std::fs::read_to_string(ca_file).map_err(|e| fail("読めません", &e))?;
    if !pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err(DriverError::new(
            ErrorKind::Connect,
            format!(
                "CA 証明書のファイルが PEM 形式ではありません: {}",
                ca_file.display()
            ),
        ));
    }
    let mut hasher = DefaultHasher::new();
    pem.hash(&mut hasher);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("sqltool-oracle-ca-{:016x}", hasher.finish()));
    std::fs::create_dir_all(&dir).map_err(|e| fail("置けません", &e))?;
    let target = dir.join("ewallet.pem");
    if std::fs::read_to_string(&target).ok().as_deref() != Some(pem.as_str()) {
        std::fs::write(&target, &pem).map_err(|e| fail("置けません", &e))?;
    }
    Ok(dir.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::wallet_dir;

    #[test]
    fn the_ca_is_placed_as_a_wallet_and_reused() {
        let dir = std::env::temp_dir().join(format!("sqltool-tls-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ca = dir.join("ca.pem");
        std::fs::write(
            &ca,
            "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let a = wallet_dir(&ca).unwrap();
        let b = wallet_dir(&ca).unwrap();
        assert_eq!(a, b);
        assert!(std::path::Path::new(&a).join("ewallet.pem").exists());
        std::fs::write(&ca, "not a certificate").unwrap();
        assert!(wallet_dir(&ca).is_err());
        assert!(wallet_dir(&dir.join("missing.pem")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
