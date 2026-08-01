// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use axum::{
    body::{Body, to_bytes},
    extract::Request,
};
use http_body_util::LengthLimitError;
use snafu::Snafu;
use std::{
    error::Error as _,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const DEFAULT_REQUESTS_PER_SECOND: f64 = 5.0;
const DEFAULT_BURST: u32 = 10;
const DEFAULT_MAX_IN_FLIGHT: usize = 16;
const DEFAULT_BODY_LIMIT_BYTES: usize = 64 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_REQUESTS_PER_SECOND: f64 = 0.1;
const MAX_REQUESTS_PER_SECOND: f64 = 1_000.0;
const MAX_BURST: u32 = 10_000;
const MAX_IN_FLIGHT: usize = 1_024;
const MIN_BODY_LIMIT_BYTES: usize = 1_024;
const MAX_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const MAX_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Snafu)]
pub(crate) enum AdmissionConfigError {
    #[snafu(display("{name} contains non-UTF-8 data"))]
    NonUtf8 { name: &'static str },

    #[snafu(display("invalid {name} value {value:?}; expected {expected}"))]
    InvalidValue {
        name: &'static str,
        value: String,
        expected: &'static str,
    },
}

#[derive(Clone, Copy)]
struct AdmissionEnvNames {
    requests_per_second: &'static str,
    burst: &'static str,
    max_in_flight: &'static str,
    body_limit_bytes: &'static str,
    timeout_seconds: &'static str,
}

/// Per-process admission settings for an unauthenticated, control-plane-backed endpoint.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AdmissionConfig {
    pub(crate) requests_per_second: f64,
    pub(crate) burst: u32,
    pub(crate) max_in_flight: usize,
    pub(crate) body_limit_bytes: usize,
    pub(crate) timeout: Duration,
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self {
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            burst: DEFAULT_BURST,
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl AdmissionConfig {
    pub(crate) fn for_endpoint(endpoint: AdmissionEndpoint) -> Self {
        match endpoint {
            AdmissionEndpoint::Sts | AdmissionEndpoint::ConsoleLogin => Self::default(),
        }
    }

    pub(crate) fn from_env(endpoint: AdmissionEndpoint) -> Result<Self, AdmissionConfigError> {
        let names = endpoint.env_names();
        let defaults = Self::for_endpoint(endpoint);
        Ok(Self {
            requests_per_second: env_f64(
                names.requests_per_second,
                defaults.requests_per_second,
                MIN_REQUESTS_PER_SECOND,
                MAX_REQUESTS_PER_SECOND,
            )?,
            burst: env_u32(names.burst, defaults.burst, 1, MAX_BURST)?,
            max_in_flight: env_usize(
                names.max_in_flight,
                defaults.max_in_flight,
                1,
                MAX_IN_FLIGHT,
            )?,
            body_limit_bytes: env_usize(
                names.body_limit_bytes,
                defaults.body_limit_bytes,
                MIN_BODY_LIMIT_BYTES,
                MAX_BODY_LIMIT_BYTES,
            )?,
            timeout: Duration::from_secs(env_u64(
                names.timeout_seconds,
                defaults.timeout.as_secs(),
                1,
                MAX_TIMEOUT_SECONDS,
            )?),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionRejection {
    RateLimited,
    ConcurrencyLimited,
    BodyTooLarge,
    BodyReadFailed,
    TimedOut,
}

impl AdmissionRejection {
    pub(crate) fn reason(self) -> AdmissionReason {
        match self {
            Self::RateLimited => AdmissionReason::RateLimit,
            Self::ConcurrencyLimited => AdmissionReason::ConcurrencyLimit,
            Self::BodyTooLarge => AdmissionReason::BodyTooLarge,
            Self::BodyReadFailed => AdmissionReason::BodyReadFailure,
            Self::TimedOut => AdmissionReason::Timeout,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum AdmissionEndpoint {
    Sts,
    ConsoleLogin,
}

impl AdmissionEndpoint {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Sts => "sts",
            Self::ConsoleLogin => "console_login",
        }
    }

    fn env_names(self) -> AdmissionEnvNames {
        match self {
            Self::Sts => AdmissionEnvNames {
                requests_per_second: "OPERATOR_STS_ADMISSION_REQUESTS_PER_SECOND",
                burst: "OPERATOR_STS_ADMISSION_BURST",
                max_in_flight: "OPERATOR_STS_ADMISSION_MAX_IN_FLIGHT",
                body_limit_bytes: "OPERATOR_STS_ADMISSION_BODY_LIMIT_BYTES",
                timeout_seconds: "OPERATOR_STS_ADMISSION_TIMEOUT_SECONDS",
            },
            Self::ConsoleLogin => AdmissionEnvNames {
                requests_per_second: "CONSOLE_LOGIN_ADMISSION_REQUESTS_PER_SECOND",
                burst: "CONSOLE_LOGIN_ADMISSION_BURST",
                max_in_flight: "CONSOLE_LOGIN_ADMISSION_MAX_IN_FLIGHT",
                body_limit_bytes: "CONSOLE_LOGIN_ADMISSION_BODY_LIMIT_BYTES",
                timeout_seconds: "CONSOLE_LOGIN_ADMISSION_TIMEOUT_SECONDS",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum AdmissionReason {
    RateLimit,
    ConcurrencyLimit,
    BodyTooLarge,
    BodyReadFailure,
    Timeout,
}

impl AdmissionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit",
            Self::ConcurrencyLimit => "concurrency_limit",
            Self::BodyTooLarge => "body_too_large",
            Self::BodyReadFailure => "body_read_failure",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: tokio::time::Instant,
}

#[derive(Debug)]
struct AdmissionInner {
    config: AdmissionConfig,
    bucket: Mutex<TokenBucket>,
    semaphore: Arc<Semaphore>,
}

/// Shared state for one endpoint. Create separate instances for STS and Console login.
#[derive(Clone, Debug)]
pub(crate) struct AdmissionControl {
    inner: Arc<AdmissionInner>,
}

impl Default for AdmissionControl {
    fn default() -> Self {
        Self::new(AdmissionConfig::default())
    }
}

impl AdmissionControl {
    pub(crate) fn new(config: AdmissionConfig) -> Self {
        debug_assert!(config.requests_per_second.is_finite());
        debug_assert!(config.requests_per_second > 0.0);
        debug_assert!(config.burst > 0);
        debug_assert!(config.max_in_flight > 0);
        debug_assert!(config.body_limit_bytes > 0);
        debug_assert!(!config.timeout.is_zero());

        let now = tokio::time::Instant::now();
        Self {
            inner: Arc::new(AdmissionInner {
                config,
                bucket: Mutex::new(TokenBucket {
                    tokens: f64::from(config.burst),
                    last_refill: now,
                }),
                semaphore: Arc::new(Semaphore::new(config.max_in_flight)),
            }),
        }
    }

    fn try_enter(&self) -> Result<OwnedSemaphorePermit, AdmissionRejection> {
        let permit = Arc::clone(&self.inner.semaphore)
            .try_acquire_owned()
            .map_err(|_| AdmissionRejection::ConcurrencyLimited)?;

        let now = tokio::time::Instant::now();
        let mut bucket = self
            .inner
            .bucket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let elapsed = now.saturating_duration_since(bucket.last_refill);
        bucket.tokens = (bucket.tokens
            + elapsed.as_secs_f64() * self.inner.config.requests_per_second)
            .min(f64::from(self.inner.config.burst));
        bucket.last_refill = now;

        if bucket.tokens < 1.0 {
            return Err(AdmissionRejection::RateLimited);
        }
        bucket.tokens -= 1.0;
        Ok(permit)
    }

    /// Run one admitted operation while holding its concurrency permit and enforcing a deadline.
    pub(crate) async fn execute<F, T>(&self, operation: F) -> Result<T, AdmissionRejection>
    where
        F: Future<Output = Result<T, AdmissionRejection>>,
    {
        let _permit = self.try_enter()?;
        tokio::time::timeout(self.inner.config.timeout, operation)
            .await
            .map_err(|_| AdmissionRejection::TimedOut)?
    }

    /// Buffer a small authentication request before its extractor can allocate an unbounded body.
    pub(crate) async fn read_bounded_request(
        &self,
        request: Request,
    ) -> Result<Request, AdmissionRejection> {
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, self.inner.config.body_limit_bytes)
            .await
            .map_err(|error| {
                if error
                    .source()
                    .is_some_and(|source| source.is::<LengthLimitError>())
                {
                    AdmissionRejection::BodyTooLarge
                } else {
                    AdmissionRejection::BodyReadFailed
                }
            })?;
        Ok(Request::from_parts(parts, Body::from(bytes)))
    }
}

fn env_value(name: &'static str) -> Result<Option<String>, AdmissionConfigError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(AdmissionConfigError::NonUtf8 { name }),
    }
}

fn env_f64(
    name: &'static str,
    default: f64,
    minimum: f64,
    maximum: f64,
) -> Result<f64, AdmissionConfigError> {
    let Some(value) = env_value(name)? else {
        return Ok(default);
    };
    let parsed = value
        .parse::<f64>()
        .ok()
        .filter(|parsed| parsed.is_finite() && (minimum..=maximum).contains(parsed))
        .ok_or_else(|| AdmissionConfigError::InvalidValue {
            name,
            value: value.clone(),
            expected: "a finite number from 0.1 through 1000",
        })?;
    Ok(parsed)
}

fn env_u32(
    name: &'static str,
    default: u32,
    minimum: u32,
    maximum: u32,
) -> Result<u32, AdmissionConfigError> {
    let Some(value) = env_value(name)? else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| (minimum..=maximum).contains(parsed))
        .ok_or(AdmissionConfigError::InvalidValue {
            name,
            value,
            expected: "an integer from 1 through 10000",
        })
}

fn env_usize(
    name: &'static str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, AdmissionConfigError> {
    let Some(value) = env_value(name)? else {
        return Ok(default);
    };
    value
        .parse::<usize>()
        .ok()
        .filter(|parsed| (minimum..=maximum).contains(parsed))
        .ok_or(AdmissionConfigError::InvalidValue {
            name,
            value,
            expected: if minimum == MIN_BODY_LIMIT_BYTES {
                "an integer from 1024 through 1048576"
            } else {
                "an integer from 1 through 1024"
            },
        })
}

fn env_u64(
    name: &'static str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64, AdmissionConfigError> {
    let Some(value) = env_value(name)? else {
        return Ok(default);
    };
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| (minimum..=maximum).contains(parsed))
        .ok_or(AdmissionConfigError::InvalidValue {
            name,
            value,
            expected: "an integer from 1 through 300",
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;

    fn test_config() -> AdmissionConfig {
        AdmissionConfig {
            requests_per_second: 0.000_001,
            burst: 2,
            max_in_flight: 1,
            body_limit_bytes: 4,
            timeout: Duration::from_millis(5),
        }
    }

    #[tokio::test]
    async fn rejects_immediately_when_burst_is_exhausted() {
        let admission = AdmissionControl::new(test_config());

        assert_eq!(admission.execute(async { Ok(()) }).await, Ok(()));
        assert_eq!(admission.execute(async { Ok(()) }).await, Ok(()));
        assert_eq!(
            admission.execute(async { Ok(()) }).await,
            Err(AdmissionRejection::RateLimited)
        );
    }

    #[tokio::test]
    async fn rejects_concurrent_work_without_consuming_an_extra_token() {
        let admission = AdmissionControl::new(test_config());
        let first = admission.try_enter().expect("first request is admitted");

        assert!(matches!(
            admission.try_enter(),
            Err(AdmissionRejection::ConcurrencyLimited)
        ));
        drop(first);
        assert!(admission.try_enter().is_ok());
    }

    #[tokio::test]
    async fn timeout_releases_the_concurrency_permit() {
        let admission = AdmissionControl::new(test_config());

        let result = admission
            .execute(pending::<Result<(), AdmissionRejection>>())
            .await;
        assert_eq!(result, Err(AdmissionRejection::TimedOut));
        assert!(admission.try_enter().is_ok());
    }

    #[tokio::test]
    async fn rejects_oversized_bodies_before_the_handler() {
        let admission = AdmissionControl::new(test_config());
        let request = Request::new(Body::from("12345"));

        assert!(matches!(
            admission.read_bounded_request(request).await,
            Err(AdmissionRejection::BodyTooLarge)
        ));
    }

    #[tokio::test]
    async fn accepts_a_body_at_the_exact_limit() {
        let admission = AdmissionControl::new(test_config());
        let request = admission
            .read_bounded_request(Request::new(Body::from("1234")))
            .await;

        assert!(request.is_ok());
    }

    #[tokio::test]
    async fn distinguishes_body_transport_failures_from_size_limits() {
        let admission = AdmissionControl::new(test_config());
        let body = Body::from_stream(futures::stream::once(async {
            Err::<axum::body::Bytes, _>(std::io::Error::other("test body failure"))
        }));

        assert!(matches!(
            admission.read_bounded_request(Request::new(body)).await,
            Err(AdmissionRejection::BodyReadFailed)
        ));
    }
}
