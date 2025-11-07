// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(unused)]
#![allow(dead_code)]

use rustls::crypto::ring::sign;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::{CertifiedKey, SigningKey};
use snafu::{ResultExt, Snafu};
use std::io::{self, BufReader};
use std::sync::Arc;

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(display("parse certificate error"))]
    InvalidCertificate { source: io::Error },

    #[snafu(display("no certificate"))]
    NonCertificate,

    #[snafu(display("parse private key error"))]
    InvalidPrivateKey { source: io::Error },

    #[snafu(display("no private key"))]
    NonPrivateKey,

    #[snafu(display("key pair match failed"))]
    MatchFailed { source: rustls::Error },

    #[snafu(display("no supported sign type"))]
    NoSupportedSignType { source: rustls::Error },
}

// 辅助函数：从 PEM 文件加载证书链
fn load_certs(cert: &[u8]) -> Result<Vec<CertificateDer<'static>>, Error> {
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert))
        .collect::<Result<Vec<CertificateDer<'static>>, _>>()
        .context(InvalidCertificateSnafu)?;

    if certs.is_empty() {
        return NonCertificateSnafu.fail();
    }

    Ok(certs)
}

fn load_private_key(private_key: &[u8]) -> Result<PrivateKeyDer<'static>, Error> {
    // rustls_pemfile::read_one() 返回一个 Option<Item>
    // Item 可以是 Key, Certificate, CSR 等
    if let Some(item) = rustls_pemfile::read_one(&mut BufReader::new(private_key))
        .context(InvalidPrivateKeySnafu)?
    {
        if let rustls_pemfile::Item::Pkcs8Key(key) = item {
            return Ok(key.into());
        }
    }

    NonPrivateKeySnafu.fail()
}

pub fn x509_key_pair<T: AsRef<[u8]>>(cert_pem: T, key_pem: T) -> Result<(), Error> {
    let certs = load_certs(cert_pem.as_ref()).unwrap();
    let private_key = load_private_key(key_pem.as_ref()).unwrap();

    let signing_key = sign::any_supported_type(&private_key).context(NoSupportedSignTypeSnafu)?;

    let certified_key = CertifiedKey::new(certs, signing_key);
    certified_key.keys_match().context(MatchFailedSnafu)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_x509_key_pair() {
        let cert_pem = "
-----BEGIN CERTIFICATE-----
MIIDuTCCAqGgAwIBAgIUc7Ajw87S6YEZj0uOBPPJUB6rT+swDQYJKoZIhvcNAQEL
BQAwbDELMAkGA1UEBhMCVVMxEzARBgNVBAgMCkNhbGlmb3JuaWExFjAUBgNVBAcM
DVNhbiBGcmFuY2lzY28xDjAMBgNVBAoMBU15T3JnMQwwCgYDVQQLDANEZXYxEjAQ
BgNVBAMMCWxvY2FsaG9zdDAeFw0yNTExMDcwOTEzMDdaFw0yNjExMDcwOTEzMDda
MGwxCzAJBgNVBAYTAlVTMRMwEQYDVQQIDApDYWxpZm9ybmlhMRYwFAYDVQQHDA1T
YW4gRnJhbmNpc2NvMQ4wDAYDVQQKDAVNeU9yZzEMMAoGA1UECwwDRGV2MRIwEAYD
VQQDDAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCv
HgyibsdMGQTScc3ch1b7rKQ2LTHszvxGHX1gP6HI+mkVRDbWLj9d7LDe2bYtr9We
EAI9yGrV+dnQRFRU54f/z0/vzT4euJj5U9gl0ui2eXqn+DPTQW/2edR8Wy3VLRwA
/v+2wAUvg3fqSMUpMYjvtpepQsxyaDbBdMyz26UKcjnlrJg3PlFdL1H8vempjqn8
nBV23O7QnVvvzZYXfeNAtrV9Okm1hRpX8ztIITZy9tvUCAIaBMw7OWz6uiD74suj
gzCCdV0fXmS1CilBLQOJT33qUCPXmVh0AQXYgDUicYZlh1FRp5NVVkhW4uR0J5Yj
rDvyO/5nQJPDbqjP56IdAgMBAAGjUzBRMB0GA1UdDgQWBBSjabEp+Qf85v7GSw/S
VQm85ImrEDAfBgNVHSMEGDAWgBSjabEp+Qf85v7GSw/SVQm85ImrEDAPBgNVHRMB
Af8EBTADAQH/MA0GCSqGSIb3DQEBCwUAA4IBAQAiLw5C62eOJGzCfsRSlyjljI+P
LL2eHb4k+lBwNfvVAjmpluoz4SjrWtcAbsrrDpQX9cV1BxPkQMO55Hqvp6MT3iwg
tuDyQiexOEtcNjzsyP/6vS+cqLf5v2QE5ZUEpQQkBo1IacwF3H7q6vWlUn55FPTd
DH9kUwousO/9Hq0S18Pfw9tlP6bAaOcuFQgcogGf6LW0UwBKfRZ7DDoh68y6Q6MK
lkGXpKL0sYnIqnMDhmZ9JgzJjqKmR8UcqDQVRfdhz5zFKh/4LRTNIwjzoyA/RU91
RsnjglGSO5HuDaFopfUkurtQiDgroZkDVZrWMJqSTphRu1se77eXa0jQGzlY
-----END CERTIFICATE-----
";

        let key_pem = "
-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCvHgyibsdMGQTS
cc3ch1b7rKQ2LTHszvxGHX1gP6HI+mkVRDbWLj9d7LDe2bYtr9WeEAI9yGrV+dnQ
RFRU54f/z0/vzT4euJj5U9gl0ui2eXqn+DPTQW/2edR8Wy3VLRwA/v+2wAUvg3fq
SMUpMYjvtpepQsxyaDbBdMyz26UKcjnlrJg3PlFdL1H8vempjqn8nBV23O7QnVvv
zZYXfeNAtrV9Okm1hRpX8ztIITZy9tvUCAIaBMw7OWz6uiD74sujgzCCdV0fXmS1
CilBLQOJT33qUCPXmVh0AQXYgDUicYZlh1FRp5NVVkhW4uR0J5YjrDvyO/5nQJPD
bqjP56IdAgMBAAECggEAD3ANOhadADLmb2zz8oCF5Qr7sQD7+T3oFIBOCLmjWBXn
RKYuVWRfVrbigsrbmhx5rwUHRY4RCQsNLiSP8Ko2nZEXoXLNCqVIaxZ+pBr7Q/bi
nsMtQm7u4WItHsdk+3mOEfJo9aHo8x7aZ++BXhfNVCCbLyNB5cYVKanTz/lJi+mP
n3FUTp/JtVLjFCS9LxWMsoEiZB16pyYBvwi8eETvZwem0w8bEisMFxv+Q3urxEam
wWLEGLCAN3uR7FuHYeTm/XrNgVjPgxt4zZ2lz2EwPXJrpql/AsgadUBAu7rNIKgr
nHk10MqKFlgr0C8a1K5DGOv6oHL6I3x5tVJ5IeMmoQKBgQDcdOtEmE1wnMijuJxa
5BhnUYjKO0Bcsl7Y20WBTChrRNAoF7HGLyqudF50Vr3oedjZ3pyuD4c3sQLflc+C
p5zx6PwSPtSQfXqfTY1OYIQdTZSPsRujW34ENupjogkXVnaNA63q3XRvu99Qyhaj
9NIrcIcS3NOhDNKvEp6lsqJM0QKBgQDLWc4IUYFbh3Vwsj/DMpvd8xvC/cLGr6PJ
a5Q40WLou1xmmBJbGiS03D6C81cnEknwf7s96QKaPAQFaRsa3Yg1p5KHVpnTqLx+
kjReDjuD52EcNqDrnRchk6kUC6NpDxMLbp5M3Wt8ALH1Yp5Yp99+/EwbTnO0PxXG
TZhFvYbjjQKBgQCZt8jIdq4gpKHeTv5u/fbqK9cGtBPnztQFv7cSNglE6qF+Iy9p
MkA/jpLB6i3XKQcEu41ibR9qvLl1L1+XCcqMf1ksW7UZ3vSemZO7H99fE1ZQbz0H
Redzhtseh8BxDm/xWaxuROZIdqZ7Db6FqlLVyUvV4jaKaIeLXZ9TiGBU0QKBgQCq
X7K0230TL9ogsuejZvqaqf4/kBcqGqySrLTCKgTB04DmYFE4zR2l/sXNN450qOkU
PCCoDVrl2JTR568TAjsGIUEubUtyv/Q1489GYoxQxoJhfg+zeKmRs0K9Dcc61aty
L5sn8XgFrBtt6dObmgMyRLaLRl7AzP40aHzFKbcjXQKBgCo5NKnS9W39uQqfdv9f
M1+pxA/G1Ytgieu5i0bdA2lB03GDc5qrpRKYM+XIWFeEQwPA+sbJ2D1UXDGfJ5ty
WhJ678vg1cGdQUTmm9KPSdoq4Cj110O2jcVkvua4i5BDFccNH7UaPqKWzn5IDcHA
xEtTaZ/YsIfm6p6Ik/oZIQAS
-----END PRIVATE KEY-----
";

        x509_key_pair(cert_pem, key_pem);
    }
}
