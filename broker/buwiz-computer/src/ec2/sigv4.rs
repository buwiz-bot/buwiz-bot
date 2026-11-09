//! AWS Signature Version 4, for the EC2 Query API: a canonical request, the string it signs, the
//! key that signs it, and the `Authorization` header. Nothing else of AWS is needed here, so this is
//! the whole of it, checked against AWS's own published test suite.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::Secret;

type HmacSha256 = Hmac<Sha256>;

/// Who signs: an access key, its secret, and the session token that goes with a short-lived one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub access_key: String,
    pub secret: Secret,
    pub token: Option<Secret>,
}

/// One request, as it will be sent. `headers` are every header that gets signed, `host` and
/// `x-amz-date` among them; `query` is the raw query, unencoded `key=value` pairs.
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: &'a [(String, String)],
    pub payload: &'a [u8],
}

/// Where and when it is signed for: `date` is `YYYYMMDDTHHMMSSZ`.
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
    pub date: &'a str,
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC takes a key of any length
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable_key());
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[allow(clippy::panic)]
fn unreachable_key() -> HmacSha256 {
    // `new_from_slice` fails only for fixed-size keys, which HMAC hasn't
    panic!("HMAC refused a key")
}

/// RFC 3986 as SigV4 has it: everything but the unreserved characters is `%XX`.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The query, each pair encoded and sorted by name, then value.
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').unwrap_or((p, ""))).map(|(k, v)| (encode(k), encode(v))).collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// The canonical request and its signed headers' names.
pub fn canonical_request(request: &Request<'_>) -> (String, String) {
    let mut headers: Vec<(String, String)> = request.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" "))).collect();
    headers.sort();
    let lines: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical = format!("{}\n{}\n{}\n{lines}\n{signed}\n{}", request.method, request.path, canonical_query(request.query), sha256_hex(request.payload));
    (canonical, signed)
}

fn credential_scope(scope: &Scope<'_>) -> String {
    format!("{}/{}/{}/aws4_request", &scope.date[..scope.date.len().min(8)], scope.region, scope.service)
}

pub fn string_to_sign(canonical: &str, scope: &Scope<'_>) -> String {
    format!("AWS4-HMAC-SHA256\n{}\n{}\n{}", scope.date, credential_scope(scope), sha256_hex(canonical.as_bytes()))
}

pub fn signing_key(secret: &Secret, scope: &Scope<'_>) -> Vec<u8> {
    let day = hmac(format!("AWS4{}", secret.expose()).as_bytes(), &scope.date.as_bytes()[..scope.date.len().min(8)]);
    let region = hmac(&day, scope.region.as_bytes());
    let service = hmac(&region, scope.service.as_bytes());
    hmac(&service, b"aws4_request")
}

/// The `Authorization` header for `request`.
pub fn authorization(credentials: &Credentials, request: &Request<'_>, scope: &Scope<'_>) -> String {
    let (canonical, signed) = canonical_request(request);
    let signature = hex(&hmac(&signing_key(&credentials.secret, scope), string_to_sign(&canonical, scope).as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={signed}, Signature={signature}", credentials.access_key, credential_scope(scope))
}

/// `YYYYMMDDTHHMMSSZ` for `unix` seconds.
pub fn amz_date(unix: u64) -> String {
    let (days, rest) = (unix / 86_400, unix % 86_400);
    let (y, m, d) = civil(days as i64);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// The calendar day `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! The cases are AWS's SigV4 test suite, as `awslabs/aws-c-auth` keeps it
    //! (`tests/aws-signing-test-suite/v4/<case>/`): its canonical requests, strings to sign and
    //! signatures, word for word.
    use super::*;

    const DATE: &str = "20150830T123600Z";

    fn example() -> Credentials {
        Credentials { access_key: "AKIDEXAMPLE".into(), secret: Secret::new("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"), token: None }
    }

    fn scope() -> Scope<'static> {
        Scope { region: "us-east-1", service: "service", date: DATE }
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn get_vanilla() {
        let headers = h(&[("Host", "example.amazonaws.com"), ("X-Amz-Date", DATE)]);
        let request = Request { method: "GET", path: "/", query: "", headers: &headers, payload: b"" };
        let (canonical, _) = canonical_request(&request);
        assert_eq!(canonical, "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(string_to_sign(&canonical, &scope()), "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\nbb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63");
        assert_eq!(
            authorization(&example(), &request, &scope()),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn get_vanilla_query_order_key_case() {
        let headers = h(&[("Host", "example.amazonaws.com"), ("X-Amz-Date", DATE)]);
        let request = Request { method: "GET", path: "/", query: "Param2=value2&Param1=value1", headers: &headers, payload: b"" };
        let (canonical, _) = canonical_request(&request);
        assert!(canonical.contains("\nParam1=value1&Param2=value2\n"), "{canonical}");
        assert_eq!(string_to_sign(&canonical, &scope()).rsplit('\n').next(), Some("816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0"));
        assert!(authorization(&example(), &request, &scope()).ends_with("Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"));
    }

    #[test]
    fn post_x_www_form_urlencoded() {
        let body = b"Param1=value1";
        let headers = h(&[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Host", "example.amazonaws.com"),
            ("Content-Length", "13"),
            ("X-Amz-Date", DATE),
            ("x-amz-content-sha256", &sha256_hex(body)),
        ]);
        let request = Request { method: "POST", path: "/", query: "", headers: &headers, payload: body };
        let (canonical, signed) = canonical_request(&request);
        assert_eq!(signed, "content-length;content-type;host;x-amz-content-sha256;x-amz-date");
        assert_eq!(string_to_sign(&canonical, &scope()).rsplit('\n').next(), Some("b1edd1d03544c25390e32085d55b57acc9a3961bb59415ff86c45c3d89d16cfb"));
        assert!(authorization(&example(), &request, &scope()).ends_with("Signature=d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b"));
    }

    #[test]
    fn post_sts_header_before() {
        let token = "AQoDYXdzEPT//////////wEXAMPLEtc764bNrC9SAPBSM22wDOk4x4HIZ8j4FZTwdQWLWsKWHGBuFqwAeMicRXmxfpSPfIeoIYRqTflfKD8YUuwthAx7mSEI/qkPpKPi/kMcGdQrmGdeehM4IC1NtBmUpp2wUE8phUZampKsburEDy0KPkyQDYwT7WZ0wq5VSXDvp75YU9HFvlRd8Tx6q6fE8YQcHNVXAkiY9q6d+xo0rKwT38xVqr7ZD0u0iPPkUL64lIZbqBAz+scqKmlzm8FDrypNC9Yjc8fPOLn9FX9KSYvKTr4rvx3iSIlTJabIQwj2ICCR/oLxBA==";
        let headers = h(&[("Host", "example.amazonaws.com"), ("X-Amz-Date", DATE), ("X-Amz-Security-Token", token)]);
        let request = Request { method: "POST", path: "/", query: "", headers: &headers, payload: b"" };
        let credentials = Credentials { token: Some(Secret::new(token)), ..example() };
        assert!(authorization(&credentials, &request, &scope()).ends_with("SignedHeaders=host;x-amz-date;x-amz-security-token, Signature=85d96828115b5dc0cfc3bd16ad9e210dd772bbebba041836c64533a82be05ead"));
    }

    #[test]
    fn the_signing_key_is_awss_worked_example() {
        // "Examples of how to derive a signing key for Signature Version 4", AWS General Reference
        let scope = Scope { region: "us-east-1", service: "iam", date: "20120215T000000Z" };
        assert_eq!(hex(&signing_key(&example().secret, &scope)), "f4780e2d9f65fa895f9c67b32ce1baf0b0d8a43505a000a1a9e090d414db404d");
    }

    #[test]
    fn dates_and_encoding() {
        assert_eq!(amz_date(1_440_938_160), DATE);
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_709_164_800), "20240229T000000Z");
        assert_eq!(encode("a b/c~d=e"), "a%20b%2Fc~d%3De");
    }
}
