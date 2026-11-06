//! A box behind TLS: a relay per service, from a port on this Mac to `<service>.<name>`.
//!
//! The far end proves itself by the one certificate a person pinned, with or without the public
//! roots' word for it: a root vouches for a name, not for the machine the person means, so no
//! token goes on that alone. The pin stands in for the name check. A certificate that renews has to
//! be trusted again. Past that it's the same bytes the box would send on loopback, so the rest of the Bot doesn't
//! know TLS is there. What it asks for by name is in `guest.rs` and the gate.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio_rustls::TlsConnector;

use crate::docker::{EXEC_PORT, HOST_PORT};
use crate::{Problem, Result, DOCKER_PORT_LAST, PORT_FIRST};

const DIAL_PATIENCE: Duration = Duration::from_secs(10);
const HANDSHAKE_PATIENCE: Duration = Duration::from_secs(10);
/// How many connections a relay carries at once.
const MAX_RELAYED: usize = 32;

pub const UNTRUSTED: &str = "This computer's certificate isn't trusted yet: test it again and check what it shows.";
const WRONG_CERTIFICATE: &str = "This computer's certificate isn't the one you trusted, so nothing was sent. If it changed on purpose, remove this computer and add it again.";
const UNREACHED: &str = "That computer can't be reached.";
const NOT_TLS: &str = "That computer didn't finish a TLS handshake.";

/// `pin` is a SHA-256 in hex.
pub fn check_pin(pin: &str) -> Result<()> {
    (pin.len() == 64 && pin.bytes().all(|b| b.is_ascii_hexdigit())).then_some(()).ok_or_else(|| Problem::Remote("That certificate fingerprint isn't a SHA-256.".to_owned()))
}

pub fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate).iter().map(|b| format!("{b:02x}")).collect()
}

/// Where a box's services are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Far {
    name: String,
    port: u16,
}

impl Far {
    /// From a checked `https://<name>[:port]`.
    pub fn parse(url: &str) -> Result<Self> {
        let at = url.strip_prefix("https://").ok_or_else(|| Problem::Failed("not an https address".to_owned()))?;
        let (name, port) = at.rsplit_once(':').map_or((at, Some(443)), |(n, p)| (n, p.parse().ok()));
        Ok(Self { name: name.to_owned(), port: port.ok_or_else(|| Problem::Failed("not a port".to_owned()))? })
    }

    /// The name a guest port goes by.
    pub fn service(&self, port: u16) -> String {
        match port {
            EXEC_PORT => format!("exec.{}", self.name),
            HOST_PORT => format!("host.{}", self.name),
            viewer => format!("desktop-{viewer}.{}", self.name),
        }
    }
}

fn provider() -> rustls::crypto::CryptoProvider {
    rustls::crypto::ring::default_provider()
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    provider().signature_verification_algorithms
}

/// Takes the certificate whose hash is `.0`, and no other. Its signature still has to hold.
#[derive(Debug)]
struct Pinned(String, WebPkiSupportedAlgorithms);

/// Notes what the far end offers and walks out: nothing is sent after it.
#[derive(Debug)]
struct Noting(Arc<Mutex<Option<String>>>, WebPkiSupportedAlgorithms);

macro_rules! signatures {
    () => {
        fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls12_signature(message, cert, dss, &self.1)
        }

        fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
            verify_tls13_signature(message, cert, dss, &self.1)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.1.supported_schemes()
        }
    };
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity).eq_ignore_ascii_case(&self.0) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
        }
    }

    signatures!();
}

impl ServerCertVerifier for Noting {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> std::result::Result<ServerCertVerified, rustls::Error> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint(end_entity));
        Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
    }

    signatures!();
}

fn builder() -> Result<rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier>> {
    ClientConfig::builder_with_provider(Arc::new(provider())).with_safe_default_protocol_versions().map_err(|e| Problem::Failed(format!("TLS: {e}")))
}

/// TLS that takes `pin`, or with none the public roots, `extra` among them, and the name.
fn config(pin: Option<&str>, extra: Option<&[u8]>) -> Result<Arc<ClientConfig>> {
    let builder = builder()?;
    Ok(Arc::new(match pin {
        Some(pin) => builder.dangerous().with_custom_certificate_verifier(Arc::new(Pinned(pin.to_owned(), algorithms()))).with_no_client_auth(),
        None => {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            if let Some(der) = extra {
                roots.add(CertificateDer::from(der.to_vec())).map_err(|e| Problem::Failed(format!("TLS: {e}")))?;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    }))
}

async fn dial(name: &str, port: u16, to: Option<SocketAddr>) -> Result<TcpStream> {
    let connecting = async {
        match to {
            Some(addr) => TcpStream::connect(addr).await,
            None => TcpStream::connect((name, port)).await,
        }
    };
    tokio::time::timeout(DIAL_PATIENCE, connecting).await.ok().and_then(|c| c.ok()).ok_or_else(|| Problem::Remote(UNREACHED.to_owned()))
}

fn server_name(name: &str) -> Result<ServerName<'static>> {
    ServerName::try_from(name.to_owned()).map_err(|_| Problem::Remote("That name can't be used for TLS.".to_owned()))
}

/// What the far end's certificate is to this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Trusted,
    /// no person has said so yet: its hash, for them to check, and whether the public roots take it
    Unknown { fingerprint: String, vouched: bool },
}

async fn handshake(name: &str, far: &Far, config: Arc<ClientConfig>, to: Option<SocketAddr>) -> Result<bool> {
    let tcp = dial(name, far.port, to).await?;
    let done = tokio::time::timeout(HANDSHAKE_PATIENCE, TlsConnector::from(config).connect(server_name(name)?, tcp)).await;
    Ok(matches!(done, Ok(Ok(_))))
}

/// Shakes hands with the box's exec service, sending nothing else. Only a pin that holds is
/// [`Seen::Trusted`].
pub async fn look(far: &Far, pin: Option<&str>, to: Option<SocketAddr>, extra_root: Option<&[u8]>) -> Result<Seen> {
    let name = far.service(EXEC_PORT);
    let shaken = handshake(&name, far, config(pin, extra_root)?, to).await?;
    if shaken && pin.is_some() {
        return Ok(Seen::Trusted);
    }
    let noted = Arc::new(Mutex::new(None));
    let noting = builder()?.dangerous().with_custom_certificate_verifier(Arc::new(Noting(noted.clone(), algorithms()))).with_no_client_auth();
    handshake(&name, far, Arc::new(noting), to).await?;
    let noted = noted.lock().unwrap_or_else(|e| e.into_inner()).take();
    match (noted, pin) {
        (None, _) => Err(Problem::Remote(NOT_TLS.to_owned())),
        (Some(fingerprint), None) => Ok(Seen::Unknown { fingerprint, vouched: shaken }),
        (Some(fingerprint), Some(pin)) if !fingerprint.eq_ignore_ascii_case(pin) => Err(Problem::Remote(WRONG_CERTIFICATE.to_owned())),
        // the certificate is right and the handshake still failed: its signature didn't hold
        (Some(_), Some(_)) => Err(Problem::Remote(NOT_TLS.to_owned())),
    }
}

/// A computer's relays. Stopped or dropped, they end, with every connection they carry.
pub struct Relays {
    stop: watch::Sender<()>,
}

impl Relays {
    /// A relay for each service of `names`: guest port → name. Comes back with guest port → port here.
    pub async fn open(far: &Far, names: &HashMap<u16, String>, pin: &str, to: Option<SocketAddr>) -> Result<(Self, HashMap<u16, u16>)> {
        let connector = TlsConnector::from(config(Some(pin), None)?);
        let (stop, stopped) = watch::channel(());
        let mut ports = HashMap::new();
        for (guest, name) in names {
            let listener = bind().await?;
            let here = listener.local_addr().map_err(|e| Problem::Failed(format!("a relay's port: {}", e.kind())))?.port();
            ports.insert(*guest, here);
            tokio::spawn(relay(listener, name.clone(), far.port, connector.clone(), to, stopped.clone()));
        }
        Ok((Self { stop }, ports))
    }

    pub fn stop(&self) {
        let _ = self.stop.send(());
    }

    /// None at all, for a test of what holds them.
    #[cfg(test)]
    pub(super) fn idle() -> Self {
        Self { stop: watch::channel(()).0 }
    }
}

/// A free port of the computers' range.
async fn bind() -> Result<TcpListener> {
    for port in (PORT_FIRST..=DOCKER_PORT_LAST).rev() {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return Ok(listener);
        }
    }
    Err(Problem::Remote("This Mac has no free ports left for a connection.".to_owned()))
}

async fn relay(listener: TcpListener, name: String, port: u16, connector: TlsConnector, to: Option<SocketAddr>, mut stopped: watch::Receiver<()>) {
    let room = Arc::new(Semaphore::new(MAX_RELAYED));
    loop {
        let accepted = tokio::select! {
            _ = stopped.changed() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((inbound, _)) = accepted else { continue };
        let Ok(permit) = room.clone().try_acquire_owned() else { continue };
        let (name, connector, mut stopped) = (name.clone(), connector.clone(), stopped.clone());
        tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = stopped.changed() => {}
                _ = carry(inbound, &name, port, connector, to) => {}
            }
        });
    }
}

async fn carry(mut inbound: TcpStream, name: &str, port: u16, connector: TlsConnector, to: Option<SocketAddr>) -> Result<()> {
    let tcp = dial(name, port, to).await?;
    let shaken = tokio::time::timeout(HANDSHAKE_PATIENCE, connector.connect(server_name(name)?, tcp)).await;
    let Ok(Ok(mut tls)) = shaken else { return Err(Problem::Remote(NOT_TLS.to_owned())) };
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut tls).await;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn each_service_has_its_name() {
        let far = Far::parse("https://box.example.com").unwrap();
        assert_eq!((far.service(EXEC_PORT).as_str(), far.port), ("exec.box.example.com", 443));
        assert_eq!(far.service(HOST_PORT), "host.box.example.com");
        assert_eq!(far.service(6902), "desktop-6902.box.example.com");
        assert_eq!(Far::parse("https://box.example.com:8443").unwrap().port, 8443);
    }

    #[test]
    fn a_pin_is_a_sha256() {
        assert!(check_pin(&"a".repeat(64)).is_ok());
        assert!(check_pin("abc").is_err());
        assert!(check_pin(&"z".repeat(64)).is_err());
    }

    #[test]
    fn only_the_pinned_certificate_passes_and_a_copy_of_the_name_doesnt_help() {
        let (right, wrong) = (CertificateDer::from(vec![1, 2, 3]), CertificateDer::from(vec![1, 2, 4]));
        let pinned = Pinned(fingerprint(&right), algorithms());
        let name = ServerName::try_from("exec.a.example").unwrap();
        assert!(pinned.verify_server_cert(&right, &[], &name, &[], UnixTime::now()).is_ok());
        assert!(pinned.verify_server_cert(&wrong, &[], &name, &[], UnixTime::now()).is_err());
    }

    #[tokio::test]
    async fn no_certificate_is_taken_on_the_far_ends_word_alone() {
        // a listener that speaks no TLS: never trusted, and said so
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let to = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            }
        });
        let far = Far::parse("https://box.example.com").unwrap();
        assert!(matches!(look(&far, None, Some(to), None).await, Err(Problem::Remote(_))));
    }
}
