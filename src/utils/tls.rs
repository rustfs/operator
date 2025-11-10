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
use rustls_pemfile::Item;
use snafu::{ResultExt, Snafu};
use std::io::{self, Cursor};

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

    #[snafu(display("no supported pem type"))]
    NoSupportedPEMType,
}

// load certificates from PEM file
fn load_certs(cert: &[u8]) -> Result<Vec<CertificateDer<'static>>, Error> {
    let certs = rustls_pemfile::certs(&mut Cursor::new(cert))
        .collect::<Result<Vec<CertificateDer<'static>>, _>>()
        .context(InvalidCertificateSnafu)?;

    if certs.is_empty() {
        return NonCertificateSnafu.fail();
    }

    Ok(certs)
}

fn load_private_key(private_key: &[u8]) -> Result<PrivateKeyDer<'static>, Error> {
    // rustls_pemfile::read_one() returns Option<Item>
    let item = rustls_pemfile::read_one(&mut Cursor::new(private_key))
        .context(InvalidPrivateKeySnafu)?
        .ok_or(Error::NonPrivateKey)?;

    // only pkcs8/pkcs1/sec1 supported
    Ok(match item {
        Item::Pkcs8Key(key) => key.into(),
        Item::Pkcs1Key(key) => key.into(),
        Item::Sec1Key(key) => key.into(),
        i => Err(Error::NoSupportedPEMType)?,
    })
}

pub fn x509_key_pair<T: AsRef<[u8]>>(cert_pem: T, key_pem: T) -> Result<(), Error> {
    let certs = load_certs(cert_pem.as_ref())?;
    let private_key = load_private_key(key_pem.as_ref())?;

    let signing_key = sign::any_supported_type(&private_key).context(NoSupportedSignTypeSnafu)?;

    let certified_key = CertifiedKey::new(certs, signing_key);
    certified_key.keys_match().context(MatchFailedSnafu)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_x509_key_pair_pkcs8() {
        let cert_pem = "
-----BEGIN CERTIFICATE-----
MIIDCTCCAfGgAwIBAgIUD4D7ObFcJ5PEZwq2t/cmrTbzcU0wDQYJKoZIhvcNAQEL
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI1MTExMDA3NDQwNVoXDTI2MTEx
MDA3NDQwNVowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAsnrreaQGztdaTppY7p1ExoDU7FpYjk8MalWs9xIioHTe
dpDlZmEWak0Q80qTvc+x6GT8VD/pLYqg6B2mot8I+Uv44GUmpPD/+WDxVbjvwL2b
fvcNGEniqKJUOy2za98WcmI8EoILwbmYy7cZslf6b3D0xuDsmovYJgtjNeziV6ie
LQfbWWXhAipYhUwaBAdUSQS+BWPPdYFG4LEE/8+BqmYdGU7ujIFlqSU89ZMfpZS4
pVRoEy16fs5O0UkbP1l63Q0qBLrLXjWw874dV8wC2p9iuVwofpDZRGhfYFaviZHb
MHdUBRUughU4vvTknAGwMzbrIH+eTp7aKrGKWb7ozQIDAQABo1MwUTAdBgNVHQ4E
FgQUGSE2L3XLbuxlA1Q0iX65aVGKzl4wHwYDVR0jBBgwFoAUGSE2L3XLbuxlA1Q0
iX65aVGKzl4wDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAGHwM
SYFN1/9ZlriVaJEpSvGlfeDvN5ipXqf0s1Ykux9rsTYchn7tcA6zhWqZUimwy/jO
I7jLfBNa3r5HT1uX3/RlMs6dMIO4h3vkSWjQ3QaGiuXh6U+erbkaeETtrw9b40ta
Dsj2rruE3Z11JV0y5fGcvXjXMFV7XsFQjNXF5TlXu4OUvfMeo9h4IbPmNQtq+g+t
nx0ZBloqo+punQVjHjovoQUWlrOOL5ZRZl1vLqqhHfw54a9weCXY8XJNnxWN0l0C
Kzht0TgbidDlWKBsk/CMTY8zpYrfVyPhnjNCeFGFG0DzrsehCgpEiEZ6vlylei7c
RfKUdp4DXmUZBDzeQw==
-----END CERTIFICATE-----
";

        let key_pem = "
-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQCyeut5pAbO11pO
mljunUTGgNTsWliOTwxqVaz3EiKgdN52kOVmYRZqTRDzSpO9z7HoZPxUP+ktiqDo
Haai3wj5S/jgZSak8P/5YPFVuO/AvZt+9w0YSeKoolQ7LbNr3xZyYjwSggvBuZjL
txmyV/pvcPTG4Oyai9gmC2M17OJXqJ4tB9tZZeECKliFTBoEB1RJBL4FY891gUbg
sQT/z4GqZh0ZTu6MgWWpJTz1kx+llLilVGgTLXp+zk7RSRs/WXrdDSoEusteNbDz
vh1XzALan2K5XCh+kNlEaF9gVq+Jkdswd1QFFS6CFTi+9OScAbAzNusgf55Ontoq
sYpZvujNAgMBAAECggEAPSmPaVNy+83jxhzxje+6AlZi4Q4C292t8QCkMdT2pcr2
82WrHz71Gf+H5/+uCnVSz8NPjyWJqFAh3PlQQe8xmZDV3Dv9lrd52MFGYqxqCMBR
OZy60ZB8SnK6b781Bang/Ni6IlOLaNtLx7/a3/lzOl5Ym5C3tCxpKXxshq3DUOtG
Qtvm43MOzkn8qBCgy/8oUcDMDjAc9THIK21TTueQkpYVAtYoXjhErzIHwisAxmWT
ZMBVufJT8J6ur+NrsoyAaBEP2DVGostiO4jzGX6JM8eFgI7f6NPT4YrO1MMV2ZvG
Lx+bkgcjiTC/Vux2yU43uS0R4Uq+d9ejj3LKSm0JBwKBgQDmapFGR76WKqjD7YH9
xvRmJzcfn1IT1Zb3qysdla5bXamSCShdeqTlnwqje6W1KCI/kACj/0zrBDwUnS+W
hkXdeJa9paZ1r8Upzf8a4LU11nbHjL6C/AISZHWaswYDusWb15FPXmpU9kp9klBt
hVx9OnpDXMXpr8dN7sM0tGWyzwKBgQDGTBoVemi6JDd+mqLNmMiVZ6APVpUC4Xp7
po8w+V+9nfxC68ZwMPp/SCgSzBNaEjnc/ACOD6ugLzCE3t0pKwohq0crrKcRSyIK
iWL9w4oOvmyEWlxQjWsHIClLvw7tYJB2jYYA/BrO337sTpWpVNB3+EQob5EPZkkd
e3skJ9DBowKBgQDJXlsF+89xN2j0ig4v9n9DA4SmSzuU//aHDn2IxnZxfOKkMQKo
53VTA/JtO7NvJdsAh943dPgI8FN9hH3BZCmMy0WaCjn24h1CUrhfCgD0QzDdZoBc
wtcgsdEh2NEp00G91+AzaAUvqWsiYQuPG5zgCIovctW4TBm3XzIUTpAOewKBgQCh
qvPtJOJzOAnCf2JSCskl/dkiCC3urlQEsbO2cumal05OZRlg6J2h3ftF7/mrCocA
Yrg1GhOLwk1lVqmq4bsd3h1lPxrqX33+Zyo8yAoroRaqBV2UEuf6ZD8m0TrjT0IY
VaO189QLa214TU15Q3u/A7rV2LfEfVkI315zCL8KzwKBgQCLo/duolgFFkO6PtTJ
pd9o2Uu8W//O8Bz7L6Rof/AwNAReLI5uPKYeUzgu6/lkQBo1vg3GneE2hbYtB4zy
v4+pApuLOStqtFz23Gj2cRYFA8uzVYHMAXs1GziUnMIRD2cIROOMu5yfq5srtZqu
7onzn/+zF+izPY4SJBe/3xGmvg==
-----END PRIVATE KEY-----
";
        assert!(matches!(
            load_private_key(key_pem.as_bytes()),
            Ok(PrivateKeyDer::Pkcs8(_))
        ));

        assert!(x509_key_pair(cert_pem, key_pem).is_ok());
    }

    #[test]
    fn test_x509_key_pair_pkcs1() {
        let cert_pem = "
-----BEGIN CERTIFICATE-----
MIIC+zCCAeOgAwIBAgIJAMrcYOlAlG6WMA0GCSqGSIb3DQEBCwUAMBQxEjAQBgNV
BAMMCWxvY2FsaG9zdDAeFw0yNTExMTAwNzU5MjFaFw0yNjExMTAwNzU5MjFaMBQx
EjAQBgNVBAMMCWxvY2FsaG9zdDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoC
ggEBAObycQwBDk+YRq5bWOjnlAx1FwwGFce90SsfzrHGpAc+K/onZuEKVVpSv1bx
MiMd3RDTe4jN0X302LbWKHUZfFgqxZV4kBpQ+O3AZujgV9c+21+Sc9SuDXWWzH06
Y5VgE+4netl68wmfril+bk5Hfs1DgGqa56w0S6gXI5SpOO6girb8Qt3IQOp9ZxLQ
+Qp9nWUqYNBzrox7wzKh11PihM4eibvbgKcpmm7W7VD5eNaAtY39EyN8z7T2+5PH
DQEZCEP+zgXxvYEIu3eRZkXQQ4cGSaa+LXYodnaxm3tfYLLQYap1isNahddT5ivU
ZboC3WY1I/xb+pw66+FUq/RVSbUCAwEAAaNQME4wHQYDVR0OBBYEFAVluFwLgDpr
yhq16CKoS853rOKaMB8GA1UdIwQYMBaAFAVluFwLgDpryhq16CKoS853rOKaMAwG
A1UdEwQFMAMBAf8wDQYJKoZIhvcNAQELBQADggEBAI5UhmHyvX3OZngBsTGt0s21
/lF4qL9GB5ZHfH5gGfsvbBsZxvNGliT6EMMhqzozHKiXF1LjOHGMt6zs+jfqpt8R
tKjgynH+hLigAKrwm789W62dIwqUyn10jJb1EIs9lF6fA5NYRwlX3mHrs1TFDBAy
53pxc9fvX6380DK+6y1xOijwNbTebExLeSBNMIEQe13GmKRBIp9lCut5UtVW4q4Z
N+xXGTzMKF9/3wjH+Qz/aYgH1Hu34NEoRKuZyQfjpuCL+hOcc4uxmBTdaI3WQ+Mp
C/zXIwDabCYpiMGtAl6lC00VEKrjwxSyIVG8eTLxJXR6/r5ywqJqypJfBWLxhMI=
-----END CERTIFICATE-----
";

        let key_pem = "
-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA5vJxDAEOT5hGrltY6OeUDHUXDAYVx73RKx/OscakBz4r+idm
4QpVWlK/VvEyIx3dENN7iM3RffTYttYodRl8WCrFlXiQGlD47cBm6OBX1z7bX5Jz
1K4NdZbMfTpjlWAT7id62XrzCZ+uKX5uTkd+zUOAaprnrDRLqBcjlKk47qCKtvxC
3chA6n1nEtD5Cn2dZSpg0HOujHvDMqHXU+KEzh6Ju9uApymabtbtUPl41oC1jf0T
I3zPtPb7k8cNARkIQ/7OBfG9gQi7d5FmRdBDhwZJpr4tdih2drGbe19gstBhqnWK
w1qF11PmK9RlugLdZjUj/Fv6nDrr4VSr9FVJtQIDAQABAoIBACPxp9aOc4O/14Bb
h0L4h/pIXwXoDIvB50Qm9yyEFhNqgb21VDXCPfaI2m7Vq0/73eQ4hgmMvwYzjWcn
fbR7+vZd8dKJqSPvZk7amymzgPhnOA1v5cc8L6wVhE4ZQFaHVZLDYkNm91yQFbMv
kktspTedQedVpKkQmpXWxBrnG41H85ncUpYb9cxjSIcCiFf+Fnv0L09Ogy30+C1t
cSY++QQYc5dGU8gJE+NyoOHyhsOpbuYh8t0ihKE+ccD83SsCTiCVEaMwBMFeCpDX
fW6UkAre7ImyCTs0C1lM90hcniK/Ngp7NTQZHCIdg6clbaFta2MEovw/bCB6WRbS
bqUexSECgYEA+L2Sd/hfMvxojcOT0mNSQU9gn0n3ikiltGb5gOXpan4zdK1xXR13
Y13v8z/GETZbx4bnWPbHibXig9D+qvFANFCdDqWbc5dEi8fctJPmhxzAhNRgaURg
vd8mKcEWf8F0iTl+wR/taek5GM83SRPWgSxyyjG1wkA0QSrdNrjMDrsCgYEA7a/t
mKOW75pTxZM6WzHzYQ2wk3VFqq2lUGRQOqD4HgzRypJ7h8U1BypwZ8gHna6YhG0P
4SZZRRlON+z7bbXoqU/c+TIdd9ukKbDb4/CB9w7xDX14ZbG/hnZoStezu9BLw1EC
ChuJnVrjcRW1sEEaKzkn/2qdLOrSbHC0wr8dWk8CgYEA+Li2yPe2WclC0t6J5Yoj
KeMxfpX7zG6wIyAExPsg17exxC3aeX2Jb/byhI10hKmSRIWEt9Sr2evhwGUvAceS
p70kDw1Rz9emVw9WhcqObPQ3HZsvfJM/GR0VkBLfaIgM+1pegMZoI8ttqH0rjwsj
Jq9HaR8j3EVO+wrdgGZwxRkCgYAHZgqHTdBM9QjWhZazcAKbasmsTWI1xeH3dqfo
q0oN5WhCXfzqZQEZkACfumJCTkUBGkP8Ri1RMVB1/TJ2X8s2Of4u45h3OqcJhS/T
EJF7F0P5n4Y35CiKDvWAHubBWeKB2euuVN0bwNCDnKFjMyOVZNoR4UezNjwGlBuM
VFadkQKBgQCntnxguWjNzL5uS9ecKCkyx/0NulE2ZTM74zW9AnWQt+0V8SSn4N7c
4G9GLjCbIoXMTEu1F2Cm5BzPbSOCWlMyte0rKW4CVpoXRfanbEfHBtNqsT/1Zk/u
OuyNA/ToGXgBsdxnvwKzATgkZVbcv5hr1QqcdATgIxMaYMIEuSTgQg==
-----END RSA PRIVATE KEY-----
";

        assert!(matches!(
            load_private_key(key_pem.as_bytes()),
            Ok(PrivateKeyDer::Pkcs1(_))
        ));

        assert!(x509_key_pair(cert_pem, key_pem).is_ok());
    }

    #[test]
    fn test_x509_key_pair_sec1() {
        let cert_pem = "
-----BEGIN CERTIFICATE-----
MIIBfDCCASOgAwIBAgIUV+itU1cpeibKyUAtc6VUrZbYl9UwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI1MTExMDA3NDcyNFoXDTI2MTExMDA3
NDcyNFowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0D
AQcDQgAEg3xBS3vFzHqayjNWmVdQgCnapYyYE14Hr8znbtFN6+P4XPhkd6ytdo0D
pyMVNy4vlS2yIvg6NmbMcDq6ugLh3KNTMFEwHQYDVR0OBBYEFBPep0F3N7xBJDFS
8JaKM2GbMtejMB8GA1UdIwQYMBaAFBPep0F3N7xBJDFS8JaKM2GbMtejMA8GA1Ud
EwEB/wQFMAMBAf8wCgYIKoZIzj0EAwIDRwAwRAIgJnBzxdAeZnCzlggFhx0sr734
3nMcd7e6AFUXCluIjH8CIHcua1Tgb+3t6lWtEa97vI6qnBxKuSCq+3R67nrX3Ph2
-----END CERTIFICATE-----
";

        let key_pem = "
-----BEGIN EC PRIVATE KEY-----
MHcCAQEEIMGNJHAp0y2fMuoq4dO57Ea2SlFvu90Einj3J2LGg3GOoAoGCCqGSM49
AwEHoUQDQgAEg3xBS3vFzHqayjNWmVdQgCnapYyYE14Hr8znbtFN6+P4XPhkd6yt
do0DpyMVNy4vlS2yIvg6NmbMcDq6ugLh3A==
-----END EC PRIVATE KEY-----
";

        assert!(matches!(
            load_private_key(key_pem.as_bytes()),
            Ok(PrivateKeyDer::Sec1(_))
        ));

        assert!(x509_key_pair(cert_pem, key_pem).is_ok());
    }
}
