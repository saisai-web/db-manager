//! TLS 接続(rustls)。既定では、証明書とホスト名を検証する。

use std::sync::Arc;

use driver_api::{ConnectionConfig, DriverError, ErrorKind, TlsMode};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, ring};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject};
use rustls::{ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use tokio_postgres_rustls::MakeRustlsConnect;

/// 接続の暗号化の方法。キャンセルの要求(別の接続)にも、同じ方法を使う。
#[derive(Clone)]
pub(crate) enum Connector {
    Plain,
    Tls(MakeRustlsConnect),
}

fn tls_error(msg: impl std::fmt::Display) -> DriverError {
    DriverError::new(
        ErrorKind::Connect,
        format!("TLS の設定に失敗しました: {msg}"),
    )
}

pub(crate) fn connector(cfg: &ConnectionConfig) -> Result<Connector, DriverError> {
    if cfg.tls == TlsMode::Disable {
        return Ok(Connector::Plain);
    }
    let provider = Arc::new(ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?;
    let config = match cfg.tls {
        TlsMode::Verify => builder
            .with_root_certificates(roots(cfg)?)
            .with_no_client_auth(),
        // 暗号化はするが、相手を確かめない。中間者に気づけないため、画面で警告する
        _ => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth(),
    };
    Ok(Connector::Tls(MakeRustlsConnect::new(config)))
}

/// OS が信頼する CA と、指定された CA ファイル。
fn roots(cfg: &ConnectionConfig) -> Result<RootCertStore, DriverError> {
    let mut store = RootCertStore::empty();
    // OS の証明書が読めなくても、CA ファイルが指定されていれば、接続できる
    let native = rustls_native_certs::load_native_certs();
    store.add_parsable_certificates(native.certs);
    if let Some(path) = &cfg.ca_file {
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|e| tls_error(format!("CA 証明書 {} を読めません: {e}", path.display())))?;
        for cert in certs {
            let cert = cert
                .map_err(|e| tls_error(format!("CA 証明書 {} が不正です: {e}", path.display())))?;
            store.add(cert).map_err(tls_error)?;
        }
    }
    if store.is_empty() {
        return Err(tls_error(
            "信頼できる CA 証明書がありません(OS の証明書を読めず、CA ファイルも指定されていません)",
        ));
    }
    Ok(store)
}

/// 証明書を検証しない。署名の形式の検証だけは行う。
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
