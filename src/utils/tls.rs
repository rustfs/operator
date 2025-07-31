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

use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use sec1::DecodeEcPrivateKey;
use std::io::Cursor;
use x509_parser::oid_registry;

#[derive(Debug)]
pub enum KeyError {
    X509Parse(String),
    PrivateKeyParse(String),
    PublicKeyParse(String),
    UnsupportedAlgorithm,
    KeyMismatch,
}

pub fn x509_key_pair<T: AsRef<[u8]>>(cert_pem: T, key_pem: T) {
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut Cursor::new(cert_pem))
        .filter_map(Result::ok)
        .map(|der| der.into_owned())
        .collect();

    if certs.is_empty() {
        panic!("aaa");
    }

    let private_key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut Cursor::new(key_pem))
            .unwrap()
            .unwrap();

    // 3. 验证密钥对是否匹配 (核心步骤)
    check_key_match(&certs[0], &private_key).unwrap();
}

fn check_key_match(
    leaf_cert: &CertificateDer,
    private_key: &PrivateKeyDer,
) -> Result<(), KeyError> {
    // 1. 解析证书
    let (_, cert) = x509_parser::parse_x509_certificate(leaf_cert.as_ref())
        .map_err(|e| KeyError::X509Parse(e.to_string()))?;
    let cert_pki = cert.public_key();

    // 2. 根据私钥类型，派生公钥并与证书中的公钥比较
    let key_matches = match private_key {
        PrivateKeyDer::Pkcs1(der) => {
            // RSA (PKCS#1 private key)
            let private_key = rsa::RsaPrivateKey::from_pkcs1_der(der.secret_pkcs1_der())
                .map_err(|e| KeyError::PrivateKeyParse(e.to_string()))?;
            let public_key_from_private = private_key.to_public_key();

            // 修正: 使用 cert_pki.raw 来解析 SubjectPublicKeyInfo
            let public_key_from_cert = rsa::RsaPublicKey::from_public_key_der(cert_pki.raw)
                .map_err(|e| KeyError::PublicKeyParse(e.to_string()))?;

            public_key_from_private == public_key_from_cert
        }
        PrivateKeyDer::Pkcs8(der) => {
            let pkcs8_der = der.secret_pkcs8_der();
            match cert_pki.algorithm.oid() {
                // RSA
                o if o.eq(&oid_registry::OID_PKCS1_RSAENCRYPTION) => {
                    let private_key = rsa::RsaPrivateKey::from_pkcs8_der(pkcs8_der)
                        .map_err(|e| KeyError::PrivateKeyParse(e.to_string()))?;

                    // 修正: 使用 from_public_key_der 解析 SubjectPublicKeyInfo
                    let cert_pubkey = rsa::RsaPublicKey::from_public_key_der(cert_pki.raw)
                        .map_err(|e| KeyError::PublicKeyParse(e.to_string()))?;

                    private_key.to_public_key() == cert_pubkey
                }
                // ECDSA P-256
                o if o.eq(&oid_registry::OID_KEY_TYPE_EC_PUBLIC_KEY) => {
                    let private_key = p256::ecdsa::SigningKey::from_pkcs8_der(pkcs8_der)
                        .map_err(|e| KeyError::PrivateKeyParse(e.to_string()))?;

                    // 修正: 从 subject_public_key.data 中获取原始公钥字节
                    let cert_pubkey = p256::ecdsa::VerifyingKey::from_sec1_bytes(
                        cert_pki.subject_public_key.data.as_ref(),
                    )
                    .map_err(|e| KeyError::PublicKeyParse(e.to_string()))?;

                    private_key.verifying_key() == &cert_pubkey
                }
                // Ed25519
                o if o.eq(&oid_registry::OID_SIG_ED25519) => {
                    // 修正: 使用 from_pkcs8_der 解析私钥 (需要开启 ed25519-dalek 的 "pkcs8" feature)
                    let private_key = ed25519_dalek::SigningKey::from_pkcs8_der(pkcs8_der)
                        .map_err(|e| KeyError::PrivateKeyParse(e.to_string()))?;

                    // 修正: 从 subject_public_key.data 获取原始公钥字节
                    let cert_pubkey = ed25519_dalek::VerifyingKey::try_from(
                        cert_pki.subject_public_key.data.as_ref(),
                    )
                    .map_err(|_| {
                        KeyError::PublicKeyParse("Invalid Ed25519 public key".to_string())
                    })?;

                    private_key.verifying_key() == cert_pubkey
                }
                _ => return Err(KeyError::UnsupportedAlgorithm),
            }
        }
        PrivateKeyDer::Sec1(der) => {
            // ECDSA (SEC1 private key)
            let private_key = p256::ecdsa::SigningKey::from_sec1_der(der.secret_sec1_der())
                .map_err(|e| KeyError::PrivateKeyParse(e.to_string()))?;

            // 修正: 从 subject_public_key.data 中获取原始公钥字节
            let cert_pubkey = p256::ecdsa::VerifyingKey::from_sec1_bytes(
                cert_pki.subject_public_key.data.as_ref(),
            )
            .map_err(|e| KeyError::PublicKeyParse(e.to_string()))?;

            private_key.verifying_key() == &cert_pubkey
        }
        // rustls-pki-types 0.22+ enum has been marked as non-exhaustive
        _ => return Err(KeyError::UnsupportedAlgorithm),
    };

    if key_matches {
        Ok(())
    } else {
        Err(KeyError::KeyMismatch)
    }
}
#[cfg(test)]
mod tests {
    use crate::utils::tls::x509_key_pair;

    #[test]
    #[ignore]
    fn test_ecdsa_x59_key_pair() {
        let pub_key = "-----BEGIN CERTIFICATE-----
MIIB/jCCAWICCQDscdUxw16XFDAJBgcqhkjOPQQBMEUxCzAJBgNVBAYTAkFVMRMw
EQYDVQQIEwpTb21lLVN0YXRlMSEwHwYDVQQKExhJbnRlcm5ldCBXaWRnaXRzIFB0
eSBMdGQwHhcNMTIxMTE0MTI0MDQ4WhcNMTUxMTE0MTI0MDQ4WjBFMQswCQYDVQQG
EwJBVTETMBEGA1UECBMKU29tZS1TdGF0ZTEhMB8GA1UEChMYSW50ZXJuZXQgV2lk
Z2l0cyBQdHkgTHRkMIGbMBAGByqGSM49AgEGBSuBBAAjA4GGAAQBY9+my9OoeSUR
lDQdV/x8LsOuLilthhiS1Tz4aGDHIPwC1mlvnf7fg5lecYpMCrLLhauAc1UJXcgl
01xoLuzgtAEAgv2P/jgytzRSpUYvgLBt1UA0leLYBy6mQQbrNEuqT3INapKIcUv8
XxYP0xMEUksLPq6Ca+CRSqTtrd/23uTnapkwCQYHKoZIzj0EAQOBigAwgYYCQXJo
A7Sl2nLVf+4Iu/tAX/IF4MavARKC4PPHK3zfuGfPR3oCCcsAoz3kAzOeijvd0iXb
H5jBImIxPL4WxQNiBTexAkF8D1EtpYuWdlVQ80/h/f4pBcGiXPqX5h2PQSQY7hP1
+jwM1FGS4fREIOvlBYr/SzzQRtwrvrzGYxDEDbsC0ZGRnA==
-----END CERTIFICATE-----";
        let pri_key = "-----BEGIN EC PARAMETERS-----
BgUrgQQAIw==
-----END EC PARAMETERS-----
-----BEGIN EC PRIVATE KEY-----
MIHcAgEBBEIBrsoKp0oqcv6/JovJJDoDVSGWdirrkgCWxrprGlzB9o0X8fV675X0
NwuBenXFfeZvVcwluO7/Q9wkYoPd/t3jGImgBwYFK4EEACOhgYkDgYYABAFj36bL
06h5JRGUNB1X/Hwuw64uKW2GGJLVPPhoYMcg/ALWaW+d/t+DmV5xikwKssuFq4Bz
VQldyCXTXGgu7OC0AQCC/Y/+ODK3NFKlRi+AsG3VQDSV4tgHLqZBBus0S6pPcg1q
kohxS/xfFg/TEwRSSws+roJr4JFKpO2t3/be5OdqmQ==
-----END EC PRIVATE KEY-----
";

        x509_key_pair(pub_key, pri_key);
    }

    #[test]
    fn test_rsa_x59_key_pair() {
        let pub_key = "-----BEGIN CERTIFICATE-----
MIIB0zCCAX2gAwIBAgIJAI/M7BYjwB+uMA0GCSqGSIb3DQEBBQUAMEUxCzAJBgNV
BAYTAkFVMRMwEQYDVQQIDApTb21lLVN0YXRlMSEwHwYDVQQKDBhJbnRlcm5ldCBX
aWRnaXRzIFB0eSBMdGQwHhcNMTIwOTEyMjE1MjAyWhcNMTUwOTEyMjE1MjAyWjBF
MQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50
ZXJuZXQgV2lkZ2l0cyBQdHkgTHRkMFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBANLJ
hPHhITqQbPklG3ibCVxwGMRfp/v4XqhfdQHdcVfHap6NQ5Wok/4xIA+ui35/MmNa
rtNuC+BdZ1tMuVCPFZcCAwEAAaNQME4wHQYDVR0OBBYEFJvKs8RfJaXTH08W+SGv
zQyKn0H8MB8GA1UdIwQYMBaAFJvKs8RfJaXTH08W+SGvzQyKn0H8MAwGA1UdEwQF
MAMBAf8wDQYJKoZIhvcNAQEFBQADQQBJlffJHybjDGxRMqaRmDhX0+6v02TUKZsW
r5QuVbpQhH6u+0UgcW0jp9QwpxoPTLTWGXEWBBBurxFwiCBhkQ+V
-----END CERTIFICATE-----";
        let pri_key = "-----BEGIN RSA PRIVATE KEY-----
MIIBOwIBAAJBANLJhPHhITqQbPklG3ibCVxwGMRfp/v4XqhfdQHdcVfHap6NQ5Wo
k/4xIA+ui35/MmNartNuC+BdZ1tMuVCPFZcCAwEAAQJAEJ2N+zsR0Xn8/Q6twa4G
6OB1M1WO+k+ztnX/1SvNeWu8D6GImtupLTYgjZcHufykj09jiHmjHx8u8ZZB/o1N
MQIhAPW+eyZo7ay3lMz1V01WVjNKK9QSn1MJlb06h/LuYv9FAiEA25WPedKgVyCW
SmUwbPw8fnTcpqDWE3yTO3vKcebqMSsCIBF3UmVue8YU3jybC3NxuXq3wNm34R8T
xVLHwDXh/6NJAiEAl2oHGGLz64BuAfjKrqwz7qMYr9HCLIe/YsoWq/olzScCIQDi
D2lWusoe2/nEqfDVVWGWlyJ7yOmqaVm/iNUN9B2N2g==
-----END RSA PRIVATE KEY-----";

        x509_key_pair(pub_key, pri_key);
    }

    #[test]
    fn test_x509_key_pair() {
        let pub_key = "-----BEGIN CERTIFICATE-----
MIIDWDCCAkCgAwIBAgIRAK0PloOwRuhi4SeSS9mjBI8wDQYJKoZIhvcNAQELBQAw
FTETMBEGA1UEAxMKa3ViZXJuZXRlczAgFw0yNTA3MDkxMjE5MTJaGA8yMTI1MDYx
NTAzMDAzNFowVDEVMBMGA1UEChMMc3lzdGVtOm5vZGVzMTswOQYDVQQDDDJzeXN0
ZW06bm9kZToqLm15bWluaW8taGwuZGVmYXVsdC5zdmMuY2x1c3Rlci5sb2NhbDBZ
MBMGByqGSM49AgEGCCqGSM49AwEHA0IABOJDvV5wv6WVUC63FmqetEzWZFJgSyVy
sJgTzEcZrYpyHDRrqHlFz339FGcAxDDORkMp+bWULI2qQIktHg0j7rqjggErMIIB
JzAOBgNVHQ8BAf8EBAMCBaAwEwYDVR0lBAwwCgYIKwYBBQUHAwEwDAYDVR0TAQH/
BAIwADAfBgNVHSMEGDAWgBTUrGt5wpreQXjCGMoAfQc93qijvDCB0AYDVR0RBIHI
MIHFgjtteW1pbmlvLXBvb2wtMC17MC4uLjJ9Lm15bWluaW8taGwuZGVmYXVsdC5z
dmMuY2x1c3Rlci5sb2NhbIIfbWluaW8uZGVmYXVsdC5zdmMuY2x1c3Rlci5sb2Nh
bIINbWluaW8uZGVmYXVsdIIRbWluaW8uZGVmYXVsdC5zdmOCJioubXltaW5pby1o
bC5kZWZhdWx0LnN2Yy5jbHVzdGVyLmxvY2FsghsqLmRlZmF1bHQuc3ZjLmNsdXN0
ZXIubG9jYWwwDQYJKoZIhvcNAQELBQADggEBAA0yDaSneHN08dbAnbyYjicwP1RW
0g5GkPEmZBj8R0WS8glxCKFSq1nLU/jXYAxF/EcmGn97NRFU4modjxTvrtR8MWOU
2f3WDc5e+qX9xTcNH+NTaI84Fx5Rpnih8cO1Sd7IfBB32Twd+AA0GDJVK56P3ZO/
sbl6Zv0rCH+L+n5PbQkN814NV+CtIpx4FnpPDItuQv1OhG2QKzk9MWruZ8yq9XEQ
BJGC65+IZUMZek1PXA5Qc/bqJZauovheY+wHyejBUGsqjHRQY9dXogCYt8kFkaSW
+l+XQboZHac+B8n1kUJW9sy2KY738V8GfUaRaQ0KQjT6VRbyFffOT4uksH4=
-----END CERTIFICATE-----";
        let pri_key = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgymB8eN7IXNKINXHi
URJC256QF+NHZ4MhaniIsCSFbeihRANCAATiQ71ecL+llVAutxZqnrRM1mRSYEsl
crCYE8xHGa2Kchw0a6h5Rc99/RRnAMQwzkZDKfm1lCyNqkCJLR4NI+66
-----END PRIVATE KEY-----";

        x509_key_pair(pub_key, pri_key);
    }
}
