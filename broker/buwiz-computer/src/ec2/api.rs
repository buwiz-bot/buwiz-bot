//! The EC2 Query API, as much of it as a computer needs: one signed `POST` an action, over TLS to
//! `ec2.<region>.amazonaws.com` and the public roots, and the XML answer read into what's asked.
//!
//! The broker's IAM user may do only this (`deploy/aws/broker-policy.json` in the computer repo):
//! describe, read a console, launch from the template with the computer's tags on the instance
//! and its volume in the same call, and start, stop and terminate what carries `buwiz:managed`.
//! So no key pair is made and nothing is tagged afterwards.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use super::creds::Keys;
use super::sigv4::{self, Scope};
use super::xml::{self, Node};
use crate::{Problem, Result};

const VERSION: &str = "2016-11-15";
const PATIENCE: Duration = Duration::from_secs(30);
const MAX_ANSWER: usize = 8 * 1024 * 1024;
pub const MANAGED_TAG: &str = "buwiz:managed";
pub const ID_TAG: &str = "buwiz:computer-id";

/// An instance, as DescribeInstances has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instance {
    pub id: String,
    /// `pending`, `running`, `stopping`, `stopped`, `shutting-down`, `terminated`
    pub state: String,
    pub public_ip: Option<String>,
    /// launched able to hibernate
    pub hibernation: bool,
    pub tags: HashMap<String, String>,
}

impl Instance {
    /// One of ours, for computer `id`: both tags, or it isn't.
    pub fn is_computer(&self, id: &str) -> bool {
        self.tags.get(MANAGED_TAG).map(String::as_str) == Some("true") && self.tags.get(ID_TAG).map(String::as_str) == Some(id)
    }
}

/// What EC2 said no with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl Refused {
    /// The keys, the policy or the rate said no: no other way of stopping would go either.
    pub fn not_about_hibernation(&self) -> bool {
        matches!(self.code.as_str(), "AuthFailure" | "InvalidClientTokenId" | "SignatureDoesNotMatch" | "ExpiredToken" | "RequestExpired" | "UnauthorizedOperation" | "AccessDenied" | "AccessDeniedException" | "RequestLimitExceeded" | "Throttling")
    }

    /// The sentence a person reads. AWS's own message stays out: its code says enough.
    pub fn problem(&self) -> Problem {
        let code = self.code.as_str();
        let credentials = |kind: &'static str, message: String| Problem::Credentials { code: kind, message };
        match code {
            "AuthFailure" | "InvalidClientTokenId" | "SignatureDoesNotMatch" => return credentials("bad_key", format!("AWS didn't take these keys ({code}). Check the access key and its secret, and this Mac's clock.")),
            "ExpiredToken" | "RequestExpired" => return credentials("expired", format!("AWS says these keys' session has ended ({code}). Sign in again, or use an access key that doesn't expire, and check this Mac's clock.")),
            "UnauthorizedOperation" | "AccessDenied" | "AccessDeniedException" => return credentials("not_allowed", format!("These AWS keys may not do that ({code}). Give their user the limited policy (Copy the limited policy), and check that the launch template, its subnet and its security group carry the tag buwiz:managed=true: a stack or module applied before that tag was needed doesn't have it, so apply it again. Then check again.")),
            "OptInRequired" => return credentials("region", "This AWS account hasn't turned this region on. Pick another region, or enable it in the AWS console.".to_owned()),
            _ => {}
        }
        Problem::Remote(match code {
            "InsufficientInstanceCapacity" | "InstanceLimitExceeded" | "VcpuLimitExceeded" => format!("AWS has no room for this computer right now ({code}). Try again later, or in another zone."),
            c if c.starts_with("InvalidLaunchTemplate") => "The launch template isn't there. Apply deploy/aws (OpenTofu) in this region first.".to_owned(),
            "RequestLimitExceeded" | "Throttling" => "AWS is turning away requests for now. Try again in a minute.".to_owned(),
            _ => format!("AWS refused that ({code})."),
        })
    }
}

/// What a call came to.
#[derive(Debug)]
pub enum Called {
    Done(Node),
    Refused(Refused),
}

/// One region's EC2, signed as the broker.
pub struct Api {
    pub region: String,
    /// `http://host:port`, plain, instead of AWS (tests)
    endpoint: Option<String>,
    keys: Arc<dyn Keys>,
    tls: Arc<rustls::ClientConfig>,
}

fn tls() -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| Problem::Failed(format!("TLS: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// A region's name, and nothing that could make a host of something else.
pub fn region_ok(region: &str) -> bool {
    let parts: Vec<&str> = region.split('-').collect();
    parts.len() >= 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

fn down(what: &str) -> Problem {
    Problem::Remote(format!("AWS can't be reached from this Mac ({what}). Check the connection and try again."))
}

impl Api {
    pub fn new(region: &str, keys: Arc<dyn Keys>) -> Result<Self> {
        if !region_ok(region) {
            return Err(Problem::Credentials { code: "region", message: format!("{region:?} isn't an AWS region, like ap-southeast-1.") });
        }
        Ok(Self { region: region.to_owned(), endpoint: None, keys, tls: tls()? })
    }

    /// Talks to `endpoint` (plain `http://host:port`) instead of AWS (tests).
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint = Some(endpoint.trim_end_matches('/').to_owned());
        self
    }

    fn host(&self) -> String {
        format!("ec2.{}.amazonaws.com", self.region)
    }

    /// `action` with `params`, signed. A refusal is an answer: [`Called::Refused`].
    pub async fn call(&self, action: &str, params: &[(String, String)]) -> Result<Called> {
        let credentials = self.keys.read()?;
        let mut pairs = vec![("Action".to_owned(), action.to_owned()), ("Version".to_owned(), VERSION.to_owned())];
        pairs.extend(params.iter().cloned());
        let body = pairs.iter().map(|(k, v)| format!("{}={}", sigv4::encode(k), sigv4::encode(v))).collect::<Vec<_>>().join("&");
        let host = match &self.endpoint {
            Some(e) => e.trim_start_matches("http://").to_owned(),
            None => self.host(),
        };
        let date = sigv4::amz_date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
        let mut headers = vec![("content-type".to_owned(), "application/x-www-form-urlencoded; charset=utf-8".to_owned()), ("host".to_owned(), host.clone()), ("x-amz-date".to_owned(), date.clone())];
        if let Some(token) = &credentials.token {
            headers.push(("x-amz-security-token".to_owned(), token.expose().to_owned()));
        }
        let signing = sigv4::Request { method: "POST", path: "/", query: "", headers: &headers, payload: body.as_bytes() };
        let authorization = sigv4::authorization(&credentials, &signing, &Scope { region: &self.region, service: "ec2", date: &date });
        let mut request = Request::builder().method("POST").uri("/");
        for (k, v) in &headers {
            request = request.header(k.as_str(), v.as_str());
        }
        let request = request.header("authorization", authorization).body(Full::new(Bytes::from(body))).map_err(|e| Problem::Failed(format!("a bad request: {e}")))?;
        let (status, answer) = tokio::time::timeout(PATIENCE, self.send(&host, request)).await.map_err(|_| down("no answer"))??;
        let text = String::from_utf8_lossy(&answer);
        let doc = xml::parse(&text)?;
        if (200..300).contains(&status) {
            return Ok(Called::Done(doc));
        }
        let error = doc.at(&["Errors", "Error"]).or_else(|| doc.child("Error"));
        let said = |k: &str| error.and_then(|e| e.text_at(&[k])).unwrap_or_default().to_owned();
        Ok(Called::Refused(Refused { status, code: said("Code"), message: said("Message") }))
    }

    /// [`Api::call`], with a refusal as its problem.
    pub async fn ask(&self, action: &str, params: &[(String, String)]) -> Result<Node> {
        match self.call(action, params).await? {
            Called::Done(node) => Ok(node),
            Called::Refused(r) => Err(r.problem()),
        }
    }

    async fn send(&self, host: &str, request: Request<Full<Bytes>>) -> Result<(u16, Bytes)> {
        let tcp = match &self.endpoint {
            Some(_) => TcpStream::connect(host).await,
            None => TcpStream::connect((host, 443)).await,
        }
        .map_err(|_| down("no connection"))?;
        if self.endpoint.is_some() {
            return exchange(TokioIo::new(tcp), request).await;
        }
        let name = rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|_| down("a bad name"))?;
        let stream = tokio_rustls::TlsConnector::from(self.tls.clone()).connect(name, tcp).await.map_err(|_| down("TLS failed"))?;
        exchange(TokioIo::new(stream), request).await
    }

    // ---- the actions ----

    pub async fn describe(&self, id: &str) -> Result<Option<Instance>> {
        match self.call("DescribeInstances", &[p("InstanceId.1", id)]).await? {
            Called::Done(doc) => Ok(instances(&doc).into_iter().find(|i| i.id == id)),
            Called::Refused(r) if r.code == "InvalidInstanceID.NotFound" || r.code == "InvalidInstanceID.Malformed" => Ok(None),
            Called::Refused(r) => Err(r.problem()),
        }
    }

    /// Every instance not yet terminated that carries both of computer `id`'s tags. Both are in the
    /// filter, and checked again here: nothing untagged is ever one of ours.
    pub async fn tagged(&self, id: &str) -> Result<Vec<Instance>> {
        let mut params = vec![p("Filter.1.Name", &format!("tag:{MANAGED_TAG}")), p("Filter.1.Value.1", "true"), p("Filter.2.Name", &format!("tag:{ID_TAG}")), p("Filter.2.Value.1", id), p("Filter.3.Name", "instance-state-name")];
        for (n, state) in ["pending", "running", "stopping", "stopped"].iter().enumerate() {
            params.push(p(&format!("Filter.3.Value.{}", n + 1), state));
        }
        let doc = self.ask("DescribeInstances", &params).await?;
        Ok(instances(&doc).into_iter().filter(|i| i.is_computer(id)).collect())
    }

    /// The template's own user data, from its default version, decoded.
    pub async fn template_user_data(&self, template: &str) -> Result<Option<String>> {
        let doc = self.ask("DescribeLaunchTemplateVersions", &[template_param(template, ""), p("Versions.1", "$Default")]).await?;
        let data = doc.items(&["launchTemplateVersionSet"]).next().and_then(|v| v.text_at(&["launchTemplateData", "userData"]).map(str::to_owned));
        match data {
            None => Ok(None),
            Some(b64) => {
                let raw = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| Problem::Failed("the template's user data isn't base64".to_owned()))?;
                Ok(Some(String::from_utf8_lossy(&raw).into_owned()))
            }
        }
    }

    /// One instance of computer `id` from the template, its tags on it and its root volume from the
    /// start. `client_token` makes a repeat of this call the same launch.
    pub async fn run(&self, template: &str, id: &str, user_data: &str, client_token: &str) -> Result<String> {
        let mut params = run_params(template, id);
        params.push(p("ClientToken", client_token));
        params.push(p("UserData", &base64::engine::general_purpose::STANDARD.encode(user_data)));
        let doc = self.ask("RunInstances", &params).await?;
        let id = doc.items(&["instancesSet"]).next().and_then(|i| i.text_at(&["instanceId"])).map(str::to_owned);
        id.ok_or_else(|| Problem::Failed("RunInstances named no instance".to_owned()))
    }

    /// A launch from the template as [`Api::run`] makes one, tags and all, sent as a dry run: AWS
    /// answers `DryRunOperation` when it would have launched, and launches nothing.
    pub async fn run_dry(&self, template: &str) -> Result<()> {
        let mut params = run_params(template, "check");
        params.push(p("DryRun", "true"));
        match self.call("RunInstances", &params).await? {
            Called::Refused(r) if r.code == "DryRunOperation" => Ok(()),
            Called::Refused(r) => Err(r.problem()),
            // a launch that went: it's ended at once, and the check doesn't pass on it
            Called::Done(doc) => {
                let ids: Vec<String> = doc.items(&["instancesSet"]).filter_map(|i| i.text_at(&["instanceId"]).map(str::to_owned)).collect();
                let ended = self.terminate(&ids).await;
                Err(Problem::Failed(match ended {
                    Ok(()) => "AWS launched an instance for what was only a check. It was terminated at once; check again.".to_owned(),
                    Err(_) => format!("AWS launched an instance for what was only a check, and it couldn't be terminated: end {} in the EC2 console.", ids.join(", ")),
                }))
            }
        }
    }

    pub async fn start(&self, id: &str) -> Result<()> {
        self.ask("StartInstances", &[p("InstanceId.1", id)]).await.map(|_| ())
    }

    /// Stops it, hibernating when `hibernate`. A refusal comes back as it is: the caller decides.
    pub async fn stop(&self, id: &str, hibernate: bool) -> Result<Called> {
        self.call("StopInstances", &[p("InstanceId.1", id), p("Hibernate", if hibernate { "true" } else { "false" })]).await
    }

    /// Terminates these. One that's already gone is fine.
    pub async fn terminate(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let params: Vec<(String, String)> = ids.iter().enumerate().map(|(n, id)| p(&format!("InstanceId.{}", n + 1), id)).collect();
        match self.call("TerminateInstances", &params).await? {
            Called::Done(_) => Ok(()),
            Called::Refused(r) if r.code == "InvalidInstanceID.NotFound" => Ok(()),
            Called::Refused(r) => Err(r.problem()),
        }
    }

    /// What it printed to its serial console, the latest of it.
    pub async fn console(&self, id: &str) -> Result<String> {
        let doc = self.ask("GetConsoleOutput", &[p("InstanceId", id), p("Latest", "true")]).await?;
        let Some(b64) = doc.text_at(&["output"]) else { return Ok(String::new()) };
        let raw = base64::engine::general_purpose::STANDARD.decode(b64.split_whitespace().collect::<String>()).map_err(|_| Problem::Failed("the console output isn't base64".to_owned()))?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    /// Whether the template is there: the keys work, and the module was applied.
    pub async fn template_exists(&self, template: &str) -> Result<()> {
        let param = if template.starts_with("lt-") { p("LaunchTemplateId.1", template) } else { p("LaunchTemplateName.1", template) };
        self.ask("DescribeLaunchTemplates", &[param]).await.map(|_| ())
    }
}

fn p(k: &str, v: &str) -> (String, String) {
    (k.to_owned(), v.to_owned())
}

/// What a launch of computer `id` from `template` says: one instance, and the tags on it and its volume.
fn run_params(template: &str, id: &str) -> Vec<(String, String)> {
    let mut params = vec![template_param(template, "LaunchTemplate."), p("MinCount", "1"), p("MaxCount", "1")];
    for (n, kind) in ["instance", "volume"].iter().enumerate() {
        let at = format!("TagSpecification.{}", n + 1);
        params.push(p(&format!("{at}.ResourceType"), kind));
        for (m, (k, v)) in tags(id).iter().enumerate() {
            params.push(p(&format!("{at}.Tag.{}.Key", m + 1), k));
            params.push(p(&format!("{at}.Tag.{}.Value", m + 1), v));
        }
    }
    params
}

fn template_param(template: &str, prefix: &str) -> (String, String) {
    if template.starts_with("lt-") {
        p(&format!("{prefix}LaunchTemplateId"), template)
    } else {
        p(&format!("{prefix}LaunchTemplateName"), template)
    }
}

/// The tags a computer's instance and volume carry.
pub fn tags(id: &str) -> [(String, String); 3] {
    [("Name".to_owned(), format!("buwiz-computer-{id}")), (MANAGED_TAG.to_owned(), "true".to_owned()), (ID_TAG.to_owned(), id.to_owned())]
}

/// Every instance in a DescribeInstances answer.
pub fn instances(doc: &Node) -> Vec<Instance> {
    let mut out = Vec::new();
    for reservation in doc.items(&["reservationSet"]) {
        for item in reservation.items(&["instancesSet"]) {
            let Some(id) = item.text_at(&["instanceId"]) else { continue };
            out.push(Instance {
                id: id.to_owned(),
                state: item.text_at(&["instanceState", "name"]).unwrap_or_default().to_owned(),
                public_ip: item.text_at(&["ipAddress"]).filter(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok()).map(str::to_owned),
                hibernation: item.text_at(&["hibernationOptions", "configured"]) == Some("true"),
                tags: item.items(&["tagSet"]).filter_map(|t| Some((t.text_at(&["key"])?.to_owned(), t.text_at(&["value"]).unwrap_or_default().to_owned()))).collect(),
            });
        }
    }
    out
}

async fn exchange<T>(io: TokioIo<T>, request: Request<Full<Bytes>>) -> Result<(u16, Bytes)>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await.map_err(|_| down("HTTP failed"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let response = sender.send_request(request).await.map_err(|_| down("the request broke off"))?;
    let status = response.status().as_u16();
    let body = Limited::new(response.into_body(), MAX_ANSWER).collect().await.map_err(|_| down("the answer broke off"))?.to_bytes();
    Ok((status, body))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// DescribeInstances as EC2 answers it, cut to what's read (shape from the API reference).
    const DESCRIBED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DescribeInstancesResponse xmlns="http://ec2.amazonaws.com/doc/2016-11-15/">
    <requestId>8f7724cf-496f-496e-8fe3-example</requestId>
    <reservationSet>
        <item>
            <reservationId>r-1234567890abcdef0</reservationId>
            <ownerId>123456789012</ownerId>
            <groupSet/>
            <instancesSet>
                <item>
                    <instanceId>i-1234567890abcdef0</instanceId>
                    <imageId>ami-bff32ccc</imageId>
                    <instanceState>
                        <code>16</code>
                        <name>running</name>
                    </instanceState>
                    <privateDnsName>ip-192-168-1-88.ap-southeast-1.compute.internal</privateDnsName>
                    <dnsName>ec2-54-194-252-215.ap-southeast-1.compute.amazonaws.com</dnsName>
                    <reason/>
                    <amiLaunchIndex>0</amiLaunchIndex>
                    <productCodes/>
                    <instanceType>m7i.large</instanceType>
                    <launchTime>2026-11-09T10:12:34.000Z</launchTime>
                    <placement><availabilityZone>ap-southeast-1a</availabilityZone><tenancy>default</tenancy></placement>
                    <monitoring><state>disabled</state></monitoring>
                    <subnetId>subnet-56f5f633</subnetId>
                    <vpcId>vpc-11112222</vpcId>
                    <privateIpAddress>192.168.1.88</privateIpAddress>
                    <ipAddress>54.194.252.215</ipAddress>
                    <tagSet>
                        <item><key>Name</key><value>buwiz-computer-cmp_ab12</value></item>
                        <item><key>buwiz:managed</key><value>true</value></item>
                        <item><key>buwiz:computer-id</key><value>cmp_ab12</value></item>
                    </tagSet>
                    <hibernationOptions><configured>true</configured></hibernationOptions>
                    <metadataOptions><state>applied</state><httpTokens>required</httpTokens><httpPutResponseHopLimit>1</httpPutResponseHopLimit><httpEndpoint>enabled</httpEndpoint><instanceMetadataTags>enabled</instanceMetadataTags></metadataOptions>
                </item>
                <item>
                    <instanceId>i-0598c7d356eba48d7</instanceId>
                    <instanceState><code>80</code><name>stopped</name></instanceState>
                    <stateReason><code>Client.UserInitiatedHibernate</code><message>Client.UserInitiatedHibernate: User initiated hibernate</message></stateReason>
                    <tagSet><item><key>buwiz:computer-id</key><value>cmp_ab12</value></item></tagSet>
                </item>
            </instancesSet>
        </item>
    </reservationSet>
</DescribeInstancesResponse>"#;

    const RAN: &str = r#"<RunInstancesResponse xmlns="http://ec2.amazonaws.com/doc/2016-11-15/">
    <requestId>7a62c49f-347e-4fc4-9331-6e8eEXAMPLE</requestId>
    <reservationId>r-1234567890abcdef0</reservationId>
    <ownerId>123456789012</ownerId>
    <groupSet/>
    <instancesSet>
      <item>
        <instanceId>i-1234567890abcdef0</instanceId>
        <imageId>ami-bff32ccc</imageId>
        <instanceState><code>0</code><name>pending</name></instanceState>
        <privateDnsName/>
        <dnsName/>
        <reason/>
        <instanceType>m7i.large</instanceType>
        <tagSet><item><key>buwiz:managed</key><value>true</value></item></tagSet>
      </item>
    </instancesSet>
</RunInstancesResponse>"#;

    #[test]
    fn describe_instances_reads_state_address_hibernation_and_tags() {
        let found = instances(&xml::parse(DESCRIBED).unwrap());
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, "i-1234567890abcdef0");
        assert_eq!(found[0].state, "running");
        assert_eq!(found[0].public_ip.as_deref(), Some("54.194.252.215"));
        assert!(found[0].hibernation);
        assert!(found[0].is_computer("cmp_ab12") && !found[0].is_computer("cmp_other"));
        assert_eq!(found[1].state, "stopped");
        assert_eq!(found[1].public_ip, None);
        assert!(!found[1].is_computer("cmp_ab12"), "the id tag alone isn't enough");
    }

    #[test]
    fn run_instances_names_its_instance() {
        let doc = xml::parse(RAN).unwrap();
        assert_eq!(doc.items(&["instancesSet"]).next().unwrap().text_at(&["instanceId"]), Some("i-1234567890abcdef0"));
    }

    #[test]
    fn a_refusal_is_our_sentence_with_awss_code() {
        let r = Refused { status: 403, code: "UnauthorizedOperation".into(), message: "You are not authorized. Encoded authorization failure message: abc".into() };
        let said = r.problem().sentence();
        assert!(said.contains("UnauthorizedOperation") && !said.contains("Encoded"), "{said}");
    }

    #[test]
    fn a_region_is_only_ever_a_region() {
        assert!(region_ok("ap-southeast-1") && region_ok("us-gov-west-1"));
        for bad in ["", "ap", "evil.com/x", "ap-southeast-1.evil.com", "AP-SOUTHEAST-1", "ap--1"] {
            assert!(!region_ok(bad), "{bad}");
        }
    }
}
